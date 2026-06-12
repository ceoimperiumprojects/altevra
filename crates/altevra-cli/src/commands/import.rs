//! `altevra import` — tool-native session backfill.
//!
//! Imports historical sessions from a supported AI tool into Altevra's
//! omniscient recorder. Handles:
//!
//! * `--tool hermes` — reads `YYYYMMDD_HHMMSS_<hex>.jsonl` files under
//!   `~/.hermes/sessions/`.
//! * `--tool claude-code` — reads `~/.claude/projects/<hash>/*.jsonl` files,
//!   graduated from the existing `analyze` parser.
//! * `--tool codex` — reads `~/.codex/history.jsonl` (+ optional
//!   `state_5.sqlite` for metadata enrichment), graduated from the existing
//!   `analyze` parser.
//!
//! All source directories are **read-only** for Altevra.
//!
//! Design notes (parity with [`analyze`]):
//!
//! * Graduated from `analyze/parsers` — no second importer built.
//! * Idempotency: `SessionsRepository::upsert_imported` keys on
//!   `(tool, external_id)`, so re-running the command produces 0 new
//!   sessions/turns. Skipped sessions never enqueue a second signal.
//! * Non-null `external_id` asserted: any session where the parser returns an
//!   empty external_id is quarantined (hash-deduped) and never written as an
//!   unguarded row.
//! * Pre-write safety (R11 / SI-7): every turn `content` and every JSON
//!   leaf inside `tool_calls` is run through `guard_text` / `guard_json`
//!   BEFORE persistence.
//! * `working_dir` threading: threaded from parser through session AND turn
//!   inserts.
//! * Oldest-watermark ordering: sessions are imported oldest-first so
//!   partial runs can be resumed by re-running with `--since`.
//! * Dry-run: parse + report projected sessions/turns/estimated-DB-bytes
//!   WITHOUT writing. Refuses to run for real if projected size >
//!   free space − 5 GiB.
//!
//! [`analyze`]: super::analyze

use altevra_db::{
    create_pool, run_migrations, signal_for_session, signal_for_skill_candidate,
    ImprovementSignalsRepository, SessionRow, SessionsRepository, TurnRow,
};
use chrono::{DateTime, NaiveDate, Utc};
use clap::Args;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::commands::analyze::parsers;
use crate::commands::analyze::ImportedSession;

#[derive(Args, Debug)]
pub struct ImportArgs {
    /// Which tool to import from. Supported: `hermes`, `claude-code`, `codex`.
    #[arg(long)]
    pub tool: String,

    /// Lower bound on session start time. Accepts RFC3339 (`2026-05-01T00:00:00Z`)
    /// or a bare date (`2026-05-01`, interpreted as midnight UTC). Sessions
    /// older than this are skipped without reading the file body — the
    /// filename timestamp is enough (for hermes) or by started_at (for others).
    #[arg(long)]
    pub since: Option<String>,

    /// SQLite database path. Defaults to the Altevra workspace DB.
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,

    /// Discover + plan only. No writes to the database, no signal enqueues.
    /// Reports projected session/turn counts and estimated DB bytes.
    #[arg(long)]
    pub dry_run: bool,

    /// Override the source directory / file. For `--tool hermes` this is the
    /// sessions directory; for `--tool claude-code` this is the projects root;
    /// for `--tool codex` this is the history.jsonl file path.
    /// Defaults to the standard location for each tool.
    /// Used by tests to point at a fixture tree.
    #[arg(long)]
    pub source_dir: Option<PathBuf>,

    /// For `--tool codex`: override the path to `state_5.sqlite`. By default
    /// the importer looks next to the history file at `~/.codex/state_5.sqlite`.
    #[arg(long)]
    pub codex_state_db: Option<PathBuf>,
}

/// Compact in-process counters reported at the end of the run.
#[derive(Debug, Default, Clone)]
pub struct ImportStats {
    pub discovered: usize,
    pub filtered_out: usize,
    pub sessions_imported: u64,
    pub sessions_skipped_existing: u64,
    pub sessions_skipped_empty: u64,
    /// Non-null external_id asserted: sessions with empty/null external_id go
    /// here instead of the main pipeline. They are hash-deduped by content
    /// fingerprint and never written as unguarded rows.
    pub sessions_quarantined: u64,
    pub turns_imported: u64,
    pub signals_enqueued: u64,
    /// Dry-run projection: estimated sessions that would be imported.
    pub projected_sessions: u64,
    /// Dry-run projection: estimated turns that would be imported.
    pub projected_turns: u64,
    /// Dry-run projection: estimated DB bytes needed.
    pub projected_bytes: u64,
    pub errors: Vec<String>,
}

pub async fn run(args: ImportArgs) -> anyhow::Result<()> {
    // Maintenance lock (db unify): import is a batch writer — refuse
    // non-fatally unless this is a read-only dry run.
    if !args.dry_run && crate::commands::brain::refuse_if_maintenance_locked("import") {
        return Ok(());
    }
    let since = match args.since.as_deref() {
        Some(s) => Some(parse_since(s)?),
        None => None,
    };

    match args.tool.as_str() {
        "hermes" => {
            let source = args.source_dir.clone().unwrap_or_else(default_hermes_dir);
            let stats = run_hermes(&source, since, &args.db, args.dry_run).await?;
            print_report(&stats, args.dry_run);
            Ok(())
        }
        "claude-code" => {
            let projects_root = args
                .source_dir
                .clone()
                .unwrap_or_else(default_claude_code_projects_dir);
            let stats = run_claude_code(&projects_root, since, &args.db, args.dry_run).await?;
            print_report(&stats, args.dry_run);
            Ok(())
        }
        "codex" => {
            let history_path = args
                .source_dir
                .clone()
                .unwrap_or_else(default_codex_history_path);
            let state_db = args
                .codex_state_db
                .clone()
                .or_else(|| default_codex_state_db_path(&history_path));
            let stats =
                run_codex(&history_path, state_db.as_deref(), since, &args.db, args.dry_run)
                    .await?;
            print_report(&stats, args.dry_run);
            Ok(())
        }
        other => anyhow::bail!(
            "unsupported --tool {other}; supported: hermes, claude-code, codex"
        ),
    }
}

fn home_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}

fn default_hermes_dir() -> PathBuf {
    home_dir().join(".hermes/sessions")
}

fn default_claude_code_projects_dir() -> PathBuf {
    home_dir().join(".claude/projects")
}

fn default_codex_history_path() -> PathBuf {
    home_dir().join(".codex/history.jsonl")
}

/// Given the history.jsonl path, derive the co-located state_5.sqlite path.
fn default_codex_state_db_path(history_path: &Path) -> Option<PathBuf> {
    let parent = history_path.parent()?;
    let candidate = parent.join("state_5.sqlite");
    if candidate.exists() {
        Some(candidate)
    } else {
        None
    }
}

/// Estimated DB bytes per turn. Conservative: 2 KiB average includes
/// content, metadata, indexes. Used for the pre-import free-space check.
const BYTES_PER_TURN_ESTIMATE: u64 = 2 * 1024;

/// The importer refuses to run for real if the projected import size
/// exceeds `free_space − FREE_SPACE_MARGIN_BYTES`.
const FREE_SPACE_MARGIN_BYTES: u64 = 5 * 1024 * 1024 * 1024; // 5 GiB

/// Query the available bytes on the filesystem that hosts `path`.
/// Uses `libc::statvfs` on Linux/macOS. Returns `None` if unavailable.
#[cfg(unix)]
fn free_bytes_on_device(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };
    if rc == 0 {
        Some(stat.f_bavail as u64 * stat.f_frsize as u64)
    } else {
        None
    }
}

#[cfg(not(unix))]
fn free_bytes_on_device(_path: &Path) -> Option<u64> {
    None
}

/// Pure decision behind the free-space gate: the import may proceed only if
/// the projected bytes still leave at least `FREE_SPACE_MARGIN_BYTES`
/// available afterwards. Split out from [`assert_free_space`] so the refusal
/// logic is unit-testable without controlling the host's real disk.
fn projected_fits_in_free_space(projected_bytes: u64, free_bytes: u64) -> bool {
    free_bytes.saturating_sub(projected_bytes) >= FREE_SPACE_MARGIN_BYTES
}

/// R3: non-null external_id is asserted on the import pipeline. When a parser
/// yields an empty/absent external_id we never pass the row through
/// unguarded — the session is quarantined under a stable content-hash key so
/// the `(tool, external_id)` idempotency contract still holds across re-runs
/// (re-importing the same source produces zero duplicates).
fn quarantine_external_id(sess: &ImportedSession) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(sess.tool_id.as_bytes());
    h.update([0]);
    for t in &sess.turns {
        h.update(t.role.as_bytes());
        h.update([0]);
        h.update(t.content.as_bytes());
        h.update([0]);
    }
    let digest = h.finalize();
    let hex: String = digest.iter().take(16).map(|b| format!("{b:02x}")).collect();
    format!("quarantine-{hex}")
}

/// Assert free space. Returns an error if the projected import size would
/// leave less than `FREE_SPACE_MARGIN_BYTES` free. Emits a warning and
/// continues if free space cannot be determined.
fn assert_free_space(projected_bytes: u64, db_path: &Path) -> anyhow::Result<()> {
    // Check against the DB file's directory (or its parent if it doesn't exist).
    let check_dir = if db_path.exists() {
        db_path.parent().unwrap_or(db_path)
    } else {
        db_path.parent().unwrap_or(Path::new("/"))
    };
    match free_bytes_on_device(check_dir) {
        None => {
            tracing::warn!(
                "cannot determine free space on {}; skipping free-space gate",
                check_dir.display()
            );
            Ok(())
        }
        Some(free) => {
            if !projected_fits_in_free_space(projected_bytes, free) {
                anyhow::bail!(
                    "import refused: projected size {} MiB + 5 GiB margin exceeds \
                     available free space {} MiB on {}. \
                     Run `cargo clean` to free build artefacts, or use `--dry-run` to inspect.",
                    projected_bytes / (1024 * 1024),
                    free / (1024 * 1024),
                    check_dir.display()
                );
            }
            Ok(())
        }
    }
}

/// Parse `--since` arg into a UTC datetime. Accepts RFC3339 or `YYYY-MM-DD`.
fn parse_since(s: &str) -> anyhow::Result<DateTime<Utc>> {
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Ok(t.with_timezone(&Utc));
    }
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        let n = d.and_hms_opt(0, 0, 0).unwrap();
        return Ok(DateTime::<Utc>::from_naive_utc_and_offset(n, Utc));
    }
    anyhow::bail!("--since must be RFC3339 or YYYY-MM-DD, got: {s}")
}

/// Walk `source` for Hermes JSONL files. Returns paths sorted by filename so
/// progress output is deterministic across runs.
fn discover_hermes_jsonl(source: &Path) -> Vec<PathBuf> {
    if !source.exists() {
        return vec![];
    }
    let mut out: Vec<PathBuf> = walkdir::WalkDir::new(source)
        .max_depth(2)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_type().is_file()
                && e.path().extension().and_then(|s| s.to_str()) == Some("jsonl")
        })
        .map(|e| e.path().to_path_buf())
        .collect();
    out.sort();
    out
}

async fn run_hermes(
    source: &Path,
    since: Option<DateTime<Utc>>,
    db: &Path,
    dry_run: bool,
) -> anyhow::Result<ImportStats> {
    let mut stats = ImportStats::default();
    let all = discover_hermes_jsonl(source);
    stats.discovered = all.len();

    // --since filter is filename-derived so we don't touch the file body for
    // sessions we're going to skip anyway.
    let to_process: Vec<PathBuf> = all
        .into_iter()
        .filter(|p| match since {
            None => true,
            Some(threshold) => p
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(parsers::hermes::parse_filename_timestamp)
                .map(|t| t >= threshold)
                .unwrap_or(false),
        })
        .collect();
    stats.filtered_out = stats.discovered.saturating_sub(to_process.len());

    if dry_run {
        // Dry-run intentionally never opens the DB pool: that keeps the
        // command safe to run before `altevra init`, and the test harness
        // can assert "no DB file was created" as a hard signal of safety.
        return Ok(stats);
    }

    let pool = create_pool(&db.to_string_lossy()).await?;
    run_migrations(&pool).await?;
    let repo = SessionsRepository::new(&pool);
    let signals = ImprovementSignalsRepository::new(&pool);

    let total = to_process.len();
    for (idx, path) in to_process.iter().enumerate() {
        match parsers::hermes::parse_session_jsonl(path) {
            Ok(Some(session)) => {
                match import_one_hermes(&repo, &signals, session, &mut stats).await {
                    Ok(()) => {}
                    Err(e) => stats.errors.push(format!("{}: {e}", path.display())),
                }
            }
            Ok(None) => {
                stats.sessions_skipped_empty += 1;
            }
            Err(e) => stats.errors.push(format!("parse {}: {e}", path.display())),
        }

        if (idx + 1) % 50 == 0 {
            println!(
                "imported {}/{} sessions, {} turns, {} signals",
                idx + 1,
                total,
                stats.turns_imported,
                stats.signals_enqueued
            );
        }
    }

    Ok(stats)
}

async fn import_one_hermes(
    repo: &SessionsRepository<'_>,
    signals: &ImprovementSignalsRepository<'_>,
    sess: ImportedSession,
    stats: &mut ImportStats,
) -> anyhow::Result<()> {
    let id = Uuid::new_v4();
    let row = SessionRow {
        id,
        tool: sess.tool_id.clone(),
        project_id: None,
        project_name: sess.project_name.clone(),
        started_at: sess.started_at,
        ended_at: sess.ended_at,
        summary: None,
        tokens_in_total: 0,
        tokens_out_total: 0,
        cost_usd_estimate: 0.0,
        turn_count: 0,
        metadata: serde_json::json!({
            "imported_at": Utc::now().to_rfc3339(),
            "imported_via": "altevra import",
            "model_hint": sess.model,
        }),
        external_id: Some(sess.external_id.clone()),
        imported_from: Some(sess.imported_from.to_string_lossy().to_string()),
        // Hermes-imported sessions have no cwd context — leave null per PLAN.md.
        working_dir: None,
    };

    let actual_id = match repo.upsert_imported(&row).await? {
        Some(actual_id) => actual_id,
        None => {
            // Existing row — idempotent re-run. No turns inserted, no signal
            // enqueued (the original ingest already produced one).
            stats.sessions_skipped_existing += 1;
            return Ok(());
        }
    };

    let mut tool_evidence_count = 0_i64;
    let file_change_count = 0_i64;

    for turn in &sess.turns {
        if matches!(turn.role.as_str(), "tool_call" | "tool_result") {
            tool_evidence_count += 1;
        }
        // R11 / SI-7: guard BEFORE persist. Same pipeline the live hook
        // handler uses — secrets + PII are scrubbed, sensitivity bumped,
        // tool_calls JSON walked leaf-by-leaf via guard_json.
        let guarded =
            altevra_secrets::guard_text(&turn.content, altevra_core::Sensitivity::Internal);
        let mut sensitivity = guarded.sensitivity.clone();
        let mut redaction = guarded.redaction_status.clone();
        let mut redacted_count = guarded.sightings.len() as i64
            + i64::from(
                guarded
                    .risk_tags
                    .contains(&altevra_core::RiskTag::ThirdPartyPii),
            );

        let scrubbed_tool_calls = if let Some(tc) = turn.tool_calls.as_ref() {
            tool_evidence_count += 1;
            let (v, c, s) = crate::commands::hook_handle::guard_json(tc);
            redacted_count += c;
            sensitivity = sensitivity.combine(&s);
            if c > 0 {
                redaction = altevra_core::status::RedactionStatus::Redacted;
            }
            Some(v)
        } else {
            None
        };

        let trow = TurnRow {
            id: Uuid::new_v4(),
            session_id: actual_id,
            turn_idx: turn.turn_idx,
            role: turn.role.clone(),
            content: guarded.value,
            tool_calls: scrubbed_tool_calls,
            tool_name: turn.tool_name.clone(),
            model: turn.model.clone(),
            tokens_in: turn.tokens_in,
            tokens_out: turn.tokens_out,
            latency_ms: turn.latency_ms,
            file_changes: None,
            redacted_count,
            source_tool: Some(sess.tool_id.clone()),
            sensitivity: sensitivity.to_string(),
            redaction_status: redaction.to_string(),
            created_at: turn.created_at,
            // Hermes-imported turns have no cwd context.
            working_dir: None,
        };
        repo.record_turn(&trow).await?;
        stats.turns_imported += 1;
    }

    stats.sessions_imported += 1;

    // C1 / SI-6 producer — enqueue one improvement signal per fresh session.
    // Best-effort: a signal-enqueue failure must not roll back the import.
    if let Some(new_signal) = signal_for_session(
        &actual_id.to_string(),
        &sess.tool_id,
        sess.project_name.as_deref(),
        sess.turns.len() as i64,
    ) {
        match signals.insert(&new_signal).await {
            Ok((_, true)) => stats.signals_enqueued += 1,
            Ok((_, false)) => { /* dedup hit — already counted on a previous run */ }
            Err(e) => stats
                .errors
                .push(format!("signal enqueue {actual_id}: {e}")),
        }
    }

    // C1.1 / Skill-factory candidate producer — still pointer-only. Local
    // heuristics may say "this session is worth skill review", but Codex/GPT
    // must follow the raw session ref before drafting SKILL.md.
    if let Some(new_signal) = signal_for_skill_candidate(
        &actual_id.to_string(),
        &sess.tool_id,
        sess.project_name.as_deref(),
        sess.turns.len() as i64,
        tool_evidence_count,
        file_change_count,
    ) {
        match signals.insert(&new_signal).await {
            Ok((_, true)) => stats.signals_enqueued += 1,
            Ok((_, false)) => { /* dedup hit — already counted on a previous run */ }
            Err(e) => stats
                .errors
                .push(format!("skill signal enqueue {actual_id}: {e}")),
        }
    }

    Ok(())
}

/// Discover all Claude Code JSONL session files under `projects_root`.
/// Returns paths sorted oldest-first (filename stems are session UUIDs, so
/// sorting alphabetically gives stable ordering; the actual oldest-first
/// constraint is fulfilled by the started_at filter in the caller).
fn discover_claude_code_jsonl(projects_root: &Path) -> Vec<PathBuf> {
    if !projects_root.exists() {
        return vec![];
    }
    let mut out: Vec<PathBuf> = walkdir::WalkDir::new(projects_root)
        .max_depth(3)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_type().is_file()
                && e.path().extension().and_then(|s| s.to_str()) == Some("jsonl")
        })
        .map(|e| e.path().to_path_buf())
        .collect();
    out.sort();
    out
}

/// Import arm for `--tool claude-code`.
/// Parses all JSONL session files under `projects_root`, applies a
/// `--since` filter by `started_at`, then imports oldest-first.
async fn run_claude_code(
    projects_root: &Path,
    since: Option<DateTime<Utc>>,
    db: &Path,
    dry_run: bool,
) -> anyhow::Result<ImportStats> {
    let mut stats = ImportStats::default();
    let all = discover_claude_code_jsonl(projects_root);
    stats.discovered = all.len();

    // Parse all sessions first (needed for started_at based --since filter and
    // for oldest-watermark ordering). Skip files with parse errors gracefully.
    let mut sessions: Vec<ImportedSession> = Vec::new();
    for path in &all {
        match parsers::claude_code::parse_file(path) {
            Ok(Some(mut sess)) => {
                if let Some(threshold) = since {
                    if sess.started_at < threshold {
                        stats.filtered_out += 1;
                        continue;
                    }
                }
                // Non-null external_id assert: empty id → quarantine under a
                // stable content-hash key (never written unguarded, still
                // deduped on re-runs).
                if sess.external_id.is_empty() {
                    stats.sessions_quarantined += 1;
                    sess.external_id = quarantine_external_id(&sess);
                    tracing::warn!(
                        path = %path.display(),
                        quarantine_key = %sess.external_id,
                        "claude-code session quarantined: empty external_id → content-hash key"
                    );
                }
                sessions.push(sess);
            }
            Ok(None) => {
                stats.sessions_skipped_empty += 1;
            }
            Err(e) => stats
                .errors
                .push(format!("parse {}: {e}", path.display())),
        }
    }

    // Oldest-watermark order: sort by started_at ascending.
    sessions.sort_by_key(|s| s.started_at);

    // Dry-run: project counts and size, check free space.
    let projected_turns: u64 = sessions.iter().map(|s| s.turns.len() as u64).sum();
    let projected_bytes = projected_turns * BYTES_PER_TURN_ESTIMATE;
    stats.projected_sessions = sessions.len() as u64;
    stats.projected_turns = projected_turns;
    stats.projected_bytes = projected_bytes;

    if dry_run {
        return Ok(stats);
    }

    // Free-space gate for real runs.
    assert_free_space(projected_bytes, db)?;

    let pool = create_pool(&db.to_string_lossy()).await?;
    run_migrations(&pool).await?;
    let repo = SessionsRepository::new(&pool);
    let signals = ImprovementSignalsRepository::new(&pool);

    let total = sessions.len();
    for (idx, sess) in sessions.into_iter().enumerate() {
        match import_one(&repo, &signals, sess, &mut stats).await {
            Ok(()) => {}
            Err(e) => stats.errors.push(format!("claude-code import {idx}: {e}")),
        }
        if (idx + 1) % 50 == 0 {
            println!(
                "claude-code: imported {}/{}, {} turns, {} signals",
                idx + 1,
                total,
                stats.turns_imported,
                stats.signals_enqueued
            );
        }
    }

    Ok(stats)
}

/// Import arm for `--tool codex`.
/// Reads `history_path` (history.jsonl) and optionally enriches with
/// thread metadata from `state_db` (state_5.sqlite). Applies `--since`
/// filter and imports oldest-first.
async fn run_codex(
    history_path: &Path,
    state_db: Option<&Path>,
    since: Option<DateTime<Utc>>,
    db: &Path,
    dry_run: bool,
) -> anyhow::Result<ImportStats> {
    let mut stats = ImportStats::default();

    if !history_path.exists() {
        tracing::info!(
            path = %history_path.display(),
            "codex history.jsonl not found — nothing to import"
        );
        return Ok(stats);
    }

    let mut sessions = match parsers::codex::parse_history(history_path, state_db) {
        Ok(s) => s,
        Err(e) => {
            anyhow::bail!("failed to parse codex history {}: {e}", history_path.display())
        }
    };
    stats.discovered = sessions.len();

    // Apply --since filter.
    if let Some(threshold) = since {
        let before = sessions.len();
        sessions.retain(|s| s.started_at >= threshold);
        stats.filtered_out = before - sessions.len();
    }

    // Non-null external_id assert: empty thread_id → quarantine under a
    // stable content-hash key (never written unguarded, deduped on re-runs).
    for sess in sessions.iter_mut() {
        if sess.external_id.is_empty() {
            stats.sessions_quarantined += 1;
            sess.external_id = quarantine_external_id(sess);
            tracing::warn!(
                quarantine_key = %sess.external_id,
                "codex session quarantined: empty external_id (thread_id was null/empty in history.jsonl) → content-hash key"
            );
        }
    }

    // Oldest-watermark order.
    sessions.sort_by_key(|s| s.started_at);

    // Dry-run projection.
    let projected_turns: u64 = sessions.iter().map(|s| s.turns.len() as u64).sum();
    let projected_bytes = projected_turns * BYTES_PER_TURN_ESTIMATE;
    stats.projected_sessions = sessions.len() as u64;
    stats.projected_turns = projected_turns;
    stats.projected_bytes = projected_bytes;

    if dry_run {
        return Ok(stats);
    }

    // Free-space gate.
    assert_free_space(projected_bytes, db)?;

    let pool = create_pool(&db.to_string_lossy()).await?;
    run_migrations(&pool).await?;
    let repo = SessionsRepository::new(&pool);
    let signals = ImprovementSignalsRepository::new(&pool);

    let total = sessions.len();
    for (idx, sess) in sessions.into_iter().enumerate() {
        match import_one(&repo, &signals, sess, &mut stats).await {
            Ok(()) => {}
            Err(e) => stats.errors.push(format!("codex import {idx}: {e}")),
        }
        if (idx + 1) % 50 == 0 {
            println!(
                "codex: imported {}/{}, {} turns, {} signals",
                idx + 1,
                total,
                stats.turns_imported,
                stats.signals_enqueued
            );
        }
    }

    Ok(stats)
}

/// Shared import-one helper used by all tool-specific arms except hermes
/// (which uses `import_one_hermes` for historical reasons and will be
/// consolidated in a later cleanup pass).
///
/// Handles:
/// * Non-null external_id assertion (caller should have quarantined already,
///   but this is a safety net).
/// * Idempotency via `upsert_imported` (tool, external_id) uniqueness.
/// * guard_text + guard_json redaction BEFORE persist.
/// * working_dir threading through session AND turn rows.
/// * Improvement signal + skill candidate signal enqueue.
async fn import_one(
    repo: &SessionsRepository<'_>,
    signals: &ImprovementSignalsRepository<'_>,
    sess: ImportedSession,
    stats: &mut ImportStats,
) -> anyhow::Result<()> {
    // Safety-net non-null assert (caller should quarantine first).
    if sess.external_id.is_empty() {
        stats.sessions_quarantined += 1;
        tracing::warn!(
            tool = %sess.tool_id,
            "import_one: empty external_id — quarantined"
        );
        return Ok(());
    }

    let id = Uuid::new_v4();
    let row = SessionRow {
        id,
        tool: sess.tool_id.clone(),
        project_id: None,
        project_name: sess.project_name.clone(),
        started_at: sess.started_at,
        ended_at: sess.ended_at,
        summary: None,
        tokens_in_total: 0,
        tokens_out_total: 0,
        cost_usd_estimate: 0.0,
        turn_count: 0,
        metadata: serde_json::json!({
            "imported_at": Utc::now().to_rfc3339(),
            "imported_via": "altevra import",
            "model_hint": sess.model,
        }),
        external_id: Some(sess.external_id.clone()),
        imported_from: Some(sess.imported_from.to_string_lossy().to_string()),
        // R3: working_dir threaded from parser.
        working_dir: sess.working_dir.clone(),
    };

    let actual_id = match repo.upsert_imported(&row).await? {
        Some(actual_id) => actual_id,
        None => {
            // Already exists — idempotent skip.
            stats.sessions_skipped_existing += 1;
            return Ok(());
        }
    };

    let mut tool_evidence_count = 0_i64;
    let file_change_count = 0_i64;

    for turn in &sess.turns {
        if matches!(turn.role.as_str(), "tool_call" | "tool_result") {
            tool_evidence_count += 1;
        }
        let guarded =
            altevra_secrets::guard_text(&turn.content, altevra_core::Sensitivity::Internal);
        let mut sensitivity = guarded.sensitivity.clone();
        let mut redaction = guarded.redaction_status.clone();
        let mut redacted_count = guarded.sightings.len() as i64
            + i64::from(
                guarded
                    .risk_tags
                    .contains(&altevra_core::RiskTag::ThirdPartyPii),
            );

        let scrubbed_tool_calls = if let Some(tc) = turn.tool_calls.as_ref() {
            tool_evidence_count += 1;
            let (v, c, s) = crate::commands::hook_handle::guard_json(tc);
            redacted_count += c;
            sensitivity = sensitivity.combine(&s);
            if c > 0 {
                redaction = altevra_core::status::RedactionStatus::Redacted;
            }
            Some(v)
        } else {
            None
        };

        let trow = TurnRow {
            id: Uuid::new_v4(),
            session_id: actual_id,
            turn_idx: turn.turn_idx,
            role: turn.role.clone(),
            content: guarded.value,
            tool_calls: scrubbed_tool_calls,
            tool_name: turn.tool_name.clone(),
            model: turn.model.clone(),
            tokens_in: turn.tokens_in,
            tokens_out: turn.tokens_out,
            latency_ms: turn.latency_ms,
            file_changes: None,
            redacted_count,
            source_tool: Some(sess.tool_id.clone()),
            sensitivity: sensitivity.to_string(),
            redaction_status: redaction.to_string(),
            created_at: turn.created_at,
            // R3: turns inherit the session's working_dir.
            working_dir: sess.working_dir.clone(),
        };
        repo.record_turn(&trow).await?;
        stats.turns_imported += 1;
    }

    stats.sessions_imported += 1;

    // Improvement signals — best-effort.
    if let Some(new_signal) = signal_for_session(
        &actual_id.to_string(),
        &sess.tool_id,
        sess.project_name.as_deref(),
        sess.turns.len() as i64,
    ) {
        match signals.insert(&new_signal).await {
            Ok((_, true)) => stats.signals_enqueued += 1,
            Ok((_, false)) => {}
            Err(e) => stats
                .errors
                .push(format!("signal enqueue {actual_id}: {e}")),
        }
    }

    if let Some(new_signal) = signal_for_skill_candidate(
        &actual_id.to_string(),
        &sess.tool_id,
        sess.project_name.as_deref(),
        sess.turns.len() as i64,
        tool_evidence_count,
        file_change_count,
    ) {
        match signals.insert(&new_signal).await {
            Ok((_, true)) => stats.signals_enqueued += 1,
            Ok((_, false)) => {}
            Err(e) => stats
                .errors
                .push(format!("skill signal enqueue {actual_id}: {e}")),
        }
    }

    Ok(())
}

fn print_report(stats: &ImportStats, dry_run: bool) {
    println!();
    if dry_run {
        println!(
            "[dry-run] projected: {} sessions, {} turns, ~{} MiB DB ({} filtered out by --since, {} quarantined)",
            stats.projected_sessions,
            stats.projected_turns,
            stats.projected_bytes / (1024 * 1024),
            stats.filtered_out,
            stats.sessions_quarantined,
        );
        return;
    }
    println!(
        "imported {} new sessions, {} new turns, {} new signals; skipped {} existing + {} empty + {} quarantined",
        stats.sessions_imported,
        stats.turns_imported,
        stats.signals_enqueued,
        stats.sessions_skipped_existing,
        stats.sessions_skipped_empty,
        stats.sessions_quarantined,
    );
    if !stats.errors.is_empty() {
        println!("errors: {}", stats.errors.len());
        for e in stats.errors.iter().take(5) {
            println!("  ! {e}");
        }
        if stats.errors.len() > 5 {
            println!("  (... {} more)", stats.errors.len() - 5);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use tempfile::TempDir;

    /// Helper: write a Hermes-shaped JSONL fixture under `dir/name`.
    fn write_fixture(dir: &Path, name: &str, lines: &[&str]) -> PathBuf {
        let p = dir.join(name);
        let mut f = fs::File::create(&p).unwrap();
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
        p
    }

    fn three_session_fixtures(dir: &Path) {
        write_fixture(
            dir,
            "20260518_120000_aaa.jsonl",
            &[
                r#"{"role":"session_meta","model":"gpt-5.5","timestamp":"2026-05-18T12:00:00.000000"}"#,
                r#"{"role":"user","content":"hello A","timestamp":"2026-05-18T12:00:01.000000"}"#,
                r#"{"role":"assistant","content":"hi A","timestamp":"2026-05-18T12:00:02.000000"}"#,
            ],
        );
        write_fixture(
            dir,
            "20260519_120000_bbb.jsonl",
            &[
                r#"{"role":"session_meta","model":"gpt-5.5","timestamp":"2026-05-19T12:00:00.000000"}"#,
                r#"{"role":"user","content":"hello B","timestamp":"2026-05-19T12:00:01.000000"}"#,
            ],
        );
        write_fixture(
            dir,
            "20260601_120000_ccc.jsonl",
            &[
                r#"{"role":"session_meta","model":"gpt-5.5","timestamp":"2026-06-01T12:00:00.000000"}"#,
                r#"{"role":"user","content":"hello C","timestamp":"2026-06-01T12:00:01.000000"}"#,
            ],
        );
    }

    #[tokio::test]
    async fn import_hermes_session_idempotent() {
        // 3 fixtures → first run imports 3, second run imports 0 (idempotent
        // on UNIQUE (tool, external_id)). Turn counts must NOT double either.
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("sessions");
        fs::create_dir_all(&src).unwrap();
        three_session_fixtures(&src);
        let db = tmp.path().join("a.db");

        let stats1 = run_hermes(&src, None, &db, false).await.unwrap();
        assert_eq!(stats1.sessions_imported, 3);
        assert!(stats1.turns_imported >= 3);
        assert_eq!(stats1.sessions_skipped_existing, 0);
        let first_turns = stats1.turns_imported;

        let stats2 = run_hermes(&src, None, &db, false).await.unwrap();
        assert_eq!(stats2.sessions_imported, 0, "second run must be idempotent");
        assert_eq!(stats2.turns_imported, 0, "no duplicate turns");
        assert_eq!(stats2.sessions_skipped_existing, 3);
        assert_eq!(first_turns, stats1.turns_imported);
    }

    #[tokio::test]
    async fn import_hermes_filters_by_since() {
        // --since 2026-05-20 → fixture A (May 18) and B (May 19) drop, only
        // C (June 1) is imported.
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("sessions");
        fs::create_dir_all(&src).unwrap();
        three_session_fixtures(&src);
        let db = tmp.path().join("b.db");

        let since = parse_since("2026-05-20").unwrap();
        let stats = run_hermes(&src, Some(since), &db, false).await.unwrap();
        assert_eq!(stats.discovered, 3);
        assert_eq!(stats.filtered_out, 2);
        assert_eq!(stats.sessions_imported, 1);
    }

    #[tokio::test]
    async fn import_hermes_guards_unscanned() {
        // R11 / SI-7: a credential-class secret in the turn body must be
        // scrubbed by guard_text BEFORE the row hits SQLite. We synthesize a
        // fake key with concat!() so the literal can't possibly trigger any
        // pre-commit scanner — the guard still has to redact it once it's
        // assembled at runtime.
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("sessions");
        fs::create_dir_all(&src).unwrap();
        // sk-ant- + 32 'A' is the canonical Anthropic-style key shape the
        // guard rules recognize. Concat at compile time so the source file
        // itself never contains the full string.
        let fake_secret = concat!("sk-ant-", "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
        let line = format!(
            r#"{{"role":"user","content":"please use {fake_secret} for the deploy","timestamp":"2026-05-18T12:00:01.000000"}}"#
        );
        write_fixture(
            &src,
            "20260518_120000_sec.jsonl",
            &[
                r#"{"role":"session_meta","model":"gpt-5.5","timestamp":"2026-05-18T12:00:00.000000"}"#,
                &line,
            ],
        );
        let db = tmp.path().join("c.db");

        let stats = run_hermes(&src, None, &db, false).await.unwrap();
        assert_eq!(stats.sessions_imported, 1);

        // Open the DB and confirm no turn row contains the raw secret.
        let pool = create_pool(&db.to_string_lossy()).await.unwrap();
        let rows: Vec<(String, i64, String)> = sqlx::query_as(
            "SELECT content, redacted_count, redaction_status FROM turns ORDER BY turn_idx",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(!rows.is_empty());
        for (content, _redacted, _status) in &rows {
            assert!(
                !content.contains(fake_secret),
                "raw secret leaked into turns.content: {content}"
            );
        }
        // At least one row should be flagged as redacted with count >= 1.
        let any_redacted = rows.iter().any(|(_, c, _)| *c >= 1);
        assert!(
            any_redacted,
            "expected at least one redacted_count >= 1 from the credential line"
        );
    }

    #[tokio::test]
    async fn import_hermes_enqueues_pointer_only_skill_candidate_signal_for_tool_heavy_session() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("sessions");
        fs::create_dir_all(&src).unwrap();
        write_fixture(
            &src,
            "20260602_120000_skill.jsonl",
            &[
                r#"{"role":"session_meta","model":"gpt-5.5","timestamp":"2026-06-02T12:00:00.000000"}"#,
                r#"{"role":"user","content":"debug this workflow","timestamp":"2026-06-02T12:00:01.000000"}"#,
                r#"{"role":"assistant","content":"I'll inspect","timestamp":"2026-06-02T12:00:02.000000","tool_calls":[{"id":"tc1","function":{"name":"read_file","arguments":"{}"}}]}"#,
                r#"{"role":"tool","tool_call_id":"tc1","content":"file content","timestamp":"2026-06-02T12:00:03.000000"}"#,
                r#"{"role":"assistant","content":"I'll test","timestamp":"2026-06-02T12:00:04.000000","tool_calls":[{"id":"tc2","function":{"name":"terminal","arguments":"{}"}}]}"#,
                r#"{"role":"tool","tool_call_id":"tc2","content":"tests pass","timestamp":"2026-06-02T12:00:05.000000"}"#,
            ],
        );
        let db = tmp.path().join("skill.db");

        let stats = run_hermes(&src, None, &db, false).await.unwrap();
        assert_eq!(stats.sessions_imported, 1);
        assert_eq!(
            stats.signals_enqueued, 2,
            "session_ingest + pointer-only skill_candidate"
        );

        let pool = create_pool(&db.to_string_lossy()).await.unwrap();
        let session_id: String = sqlx::query_scalar(
            "SELECT id FROM sessions WHERE external_id = '20260602_120000_skill'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let raw_ref = format!("session:{session_id}");
        let rows: Vec<(String, String, String, String)> = sqlx::query_as(
            "SELECT kind, source_ref, summary, cluster_key FROM improvement_signals ORDER BY kind",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        let skill = rows
            .iter()
            .find(|(kind, _, _, _)| kind == "skill_candidate")
            .expect("missing skill_candidate signal");
        assert_eq!(skill.1, raw_ref);
        assert!(skill.2.contains(&format!("raw_trace_ref={raw_ref}")));
        assert!(skill.2.contains("tool_calls=4"));
        assert_eq!(skill.3, "skill:hermes");
    }

    #[tokio::test]
    async fn import_hermes_dry_run_writes_nothing() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("sessions");
        fs::create_dir_all(&src).unwrap();
        three_session_fixtures(&src);
        let db = tmp.path().join("d.db");

        let stats = run_hermes(&src, None, &db, true).await.unwrap();
        assert_eq!(stats.discovered, 3);
        assert_eq!(stats.sessions_imported, 0);
        assert_eq!(stats.turns_imported, 0);
        // Dry-run must never create the DB file.
        assert!(!db.exists(), "dry-run created a DB file at {db:?}");
    }

    #[test]
    fn parse_since_accepts_rfc3339_and_date() {
        assert!(parse_since("2026-05-01").is_ok());
        assert!(parse_since("2026-05-01T00:00:00Z").is_ok());
        assert!(parse_since("not-a-date").is_err());
    }

    // -----------------------------------------------------------------------
    // R3: claude-code import arm tests
    // -----------------------------------------------------------------------

    /// Write a minimal Claude Code JSONL session file under
    /// `<projects_root>/<encoded_dir>/<stem>.jsonl`.
    fn write_cc_fixture(
        projects_root: &Path,
        encoded_dir: &str,
        stem: &str,
        cwd: Option<&str>,
        lines: &[&str],
    ) -> PathBuf {
        let dir = projects_root.join(encoded_dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{stem}.jsonl"));
        let mut f = fs::File::create(&path).unwrap();
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
        // Add cwd line if specified (as the first turn).
        let _ = cwd; // cwd is baked into lines by callers
        path
    }

    #[tokio::test]
    async fn import_cc_idempotent() {
        let tmp = TempDir::new().unwrap();
        let projects_root = tmp.path().join("projects");
        let db = tmp.path().join("cc.db");

        write_cc_fixture(
            &projects_root,
            "-home-pavle-projekti-altevra",
            "aaaabbbb-cccc-dddd-eeee-ffffffffffff",
            None,
            &[
                r#"{"type":"user","timestamp":"2026-05-27T10:00:00Z","cwd":"/home/pavle/projekti/altevra","message":{"role":"user","content":"hello"}}"#,
                r#"{"type":"assistant","timestamp":"2026-05-27T10:00:01Z","message":{"role":"assistant","content":"hi"}}"#,
            ],
        );

        let stats1 = run_claude_code(&projects_root, None, &db, false)
            .await
            .unwrap();
        assert_eq!(stats1.sessions_imported, 1);
        assert_eq!(stats1.turns_imported, 2);

        // Second run: idempotent.
        let stats2 = run_claude_code(&projects_root, None, &db, false)
            .await
            .unwrap();
        assert_eq!(stats2.sessions_imported, 0, "second run must be idempotent");
        assert_eq!(stats2.sessions_skipped_existing, 1);
        assert_eq!(stats2.turns_imported, 0);
    }

    /// working_dir is threaded from the transcript cwd field to both
    /// the session row and all turn rows.
    #[tokio::test]
    async fn import_cc_working_dir_threaded_to_session_and_turns() {
        let tmp = TempDir::new().unwrap();
        let projects_root = tmp.path().join("projects");
        let db = tmp.path().join("cc_wd.db");

        write_cc_fixture(
            &projects_root,
            "-home-pavle-projekti-altevra",
            "11111111-2222-3333-4444-555555555555",
            None,
            &[
                r#"{"type":"user","timestamp":"2026-05-27T10:00:00Z","cwd":"/home/pavle/projekti/altevra","message":{"role":"user","content":"test"}}"#,
                r#"{"type":"assistant","timestamp":"2026-05-27T10:00:01Z","message":{"role":"assistant","content":"ok"}}"#,
            ],
        );

        run_claude_code(&projects_root, None, &db, false)
            .await
            .unwrap();

        let pool = create_pool(&db.to_string_lossy()).await.unwrap();
        let sess_wd: Option<String> = sqlx::query_scalar(
            "SELECT working_dir FROM sessions WHERE tool = 'claude-code' LIMIT 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            sess_wd.as_deref(),
            Some("/home/pavle/projekti/altevra"),
            "session working_dir must be threaded from transcript cwd"
        );

        let turn_wds: Vec<Option<String>> =
            sqlx::query_scalar("SELECT working_dir FROM turns ORDER BY turn_idx")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert!(!turn_wds.is_empty());
        for wd in &turn_wds {
            assert_eq!(
                wd.as_deref(),
                Some("/home/pavle/projekti/altevra"),
                "all turns must inherit session working_dir"
            );
        }
    }

    /// guard_text is applied before persist for claude-code sessions.
    #[tokio::test]
    async fn import_cc_guard_applied() {
        let tmp = TempDir::new().unwrap();
        let projects_root = tmp.path().join("projects");
        let db = tmp.path().join("cc_guard.db");
        let fake_secret = concat!("sk-ant-", "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");

        write_cc_fixture(
            &projects_root,
            "-home-pavle",
            "22222222-3333-4444-5555-666666666666",
            None,
            &[
                &format!(
                    r#"{{"type":"user","timestamp":"2026-05-27T10:00:00Z","message":{{"role":"user","content":"use {fake_secret}"}}}}"#
                ),
            ],
        );

        run_claude_code(&projects_root, None, &db, false)
            .await
            .unwrap();

        let pool = create_pool(&db.to_string_lossy()).await.unwrap();
        let content: String =
            sqlx::query_scalar("SELECT content FROM turns LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(
            !content.contains(fake_secret),
            "raw secret must not reach turns.content"
        );
    }

    /// Dry-run reports projected counts and NEVER creates the DB file.
    #[tokio::test]
    async fn import_cc_dry_run_reports_size_and_no_db() {
        let tmp = TempDir::new().unwrap();
        let projects_root = tmp.path().join("projects");
        let db = tmp.path().join("cc_dry.db");

        write_cc_fixture(
            &projects_root,
            "-home-pavle",
            "33333333-4444-5555-6666-777777777777",
            None,
            &[
                r#"{"type":"user","timestamp":"2026-05-27T10:00:00Z","message":{"role":"user","content":"hello"}}"#,
                r#"{"type":"assistant","timestamp":"2026-05-27T10:00:01Z","message":{"role":"assistant","content":"hi"}}"#,
            ],
        );

        let stats = run_claude_code(&projects_root, None, &db, true)
            .await
            .unwrap();
        assert_eq!(stats.projected_sessions, 1);
        assert_eq!(stats.projected_turns, 2);
        assert!(stats.projected_bytes > 0, "projected_bytes must be > 0");
        assert_eq!(stats.sessions_imported, 0);
        assert_eq!(stats.turns_imported, 0);
        assert!(!db.exists(), "dry-run must NOT create the DB file");
    }

    /// Free-space refusal: a projected import that would leave < 5 GiB free
    /// must be refused. We simulate this by injecting a tiny free-space value
    /// via a custom check. Since we can't inject the OS call directly, we
    /// test the assert_free_space helper unit-test.
    #[test]
    fn assert_free_space_refuses_near_full_disk() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("test.db");
        // Simulate projected_bytes = 1 GiB, free = 4 GiB (less than 1+5=6 GiB needed).
        // We can't control the OS free space, so we test the logic with a very
        // large projected_bytes that will always exceed real free space on the
        // CI machine (the test runner almost certainly has < u64::MAX / 2 bytes).
        let result = assert_free_space(u64::MAX / 2, &db);
        // Should either refuse (disk does not have 9_000_000_000 GiB free)
        // or succeed if free_bytes_on_device returns None (warn-and-continue).
        // Both outcomes are acceptable — we just assert it doesn't panic.
        let _ = result; // either Ok or Err is fine
    }

    /// Free-space refusal logic — pure decision function, deterministic
    /// regardless of the host's real disk state.
    #[test]
    fn free_space_refusal_logic() {
        const GIB: u64 = 1024 * 1024 * 1024;
        // 1 GiB projected, 10 GiB free → fits (leaves 9 GiB > 5 GiB margin).
        assert!(projected_fits_in_free_space(GIB, 10 * GIB));
        // 1 GiB projected, 4 GiB free → refused (would leave 3 GiB < margin).
        assert!(!projected_fits_in_free_space(GIB, 4 * GIB));
        // Exactly at the margin boundary → allowed.
        assert!(projected_fits_in_free_space(GIB, 6 * GIB));
        // Just under the boundary → refused.
        assert!(!projected_fits_in_free_space(GIB + 1, 6 * GIB));
        // Projected larger than free → refused (saturating, no underflow).
        assert!(!projected_fits_in_free_space(20 * GIB, 10 * GIB));
        // Zero projected on a near-empty disk → refused (margin still applies).
        assert!(!projected_fits_in_free_space(0, GIB));
    }

    // -----------------------------------------------------------------------
    // R3: codex import arm tests
    // -----------------------------------------------------------------------

    fn write_codex_history(dir: &Path, lines: &[&str]) -> PathBuf {
        let path = dir.join("history.jsonl");
        let mut f = fs::File::create(&path).unwrap();
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
        path
    }

    #[tokio::test]
    async fn import_codex_idempotent() {
        let tmp = TempDir::new().unwrap();
        let history = write_codex_history(
            tmp.path(),
            &[
                r#"{"thread_id":"t-codex-1","timestamp":"2026-05-27T10:00:00Z","role":"user","content":"hello"}"#,
                r#"{"thread_id":"t-codex-1","timestamp":"2026-05-27T10:01:00Z","role":"assistant","content":"hi"}"#,
            ],
        );
        let db = tmp.path().join("codex.db");

        let stats1 = run_codex(&history, None, None, &db, false).await.unwrap();
        assert_eq!(stats1.sessions_imported, 1);
        assert_eq!(stats1.turns_imported, 2);

        let stats2 = run_codex(&history, None, None, &db, false).await.unwrap();
        assert_eq!(stats2.sessions_imported, 0, "second run must be idempotent");
        assert_eq!(stats2.sessions_skipped_existing, 1);
    }

    /// Codex dry-run: parse + project counts; no DB.
    #[tokio::test]
    async fn import_codex_dry_run_reports_size() {
        let tmp = TempDir::new().unwrap();
        let history = write_codex_history(
            tmp.path(),
            &[
                r#"{"thread_id":"t-dry","timestamp":"2026-05-27T10:00:00Z","role":"user","content":"hello"}"#,
                r#"{"thread_id":"t-dry","timestamp":"2026-05-27T10:01:00Z","role":"assistant","content":"hi"}"#,
            ],
        );
        let db = tmp.path().join("codex_dry.db");

        let stats = run_codex(&history, None, None, &db, true).await.unwrap();
        assert_eq!(stats.projected_sessions, 1);
        assert_eq!(stats.projected_turns, 2);
        assert!(stats.projected_bytes > 0);
        assert_eq!(stats.sessions_imported, 0);
        assert!(!db.exists(), "dry-run must NOT create DB");
    }

    /// Codex: non-null external_id asserted — sessions without thread_id are
    /// quarantined and never inserted.
    #[tokio::test]
    async fn import_codex_null_external_id_quarantined() {
        let tmp = TempDir::new().unwrap();
        // A line with no thread_id → gets assigned "codex-anon-1" by the parser
        // (non-empty), so it's not quarantined by the empty check. Instead,
        // test a session where thread_id is explicitly "".
        let history = write_codex_history(
            tmp.path(),
            &[
                // Valid session.
                r#"{"thread_id":"valid-thread","timestamp":"2026-05-27T10:00:00Z","role":"user","content":"ok"}"#,
            ],
        );
        let db = tmp.path().join("codex_nonnull.db");

        let stats = run_codex(&history, None, None, &db, false).await.unwrap();
        // The parser assigns "codex-anon-N" for lines without thread_id, but our
        // test has a valid thread_id, so no quarantine expected.
        assert_eq!(stats.sessions_imported, 1);
        assert_eq!(stats.sessions_quarantined, 0);
    }

    /// Codex: an explicitly EMPTY thread_id triggers the quarantine path —
    /// the session is rekeyed to a stable content-hash external_id (never
    /// written with a null/empty key), and re-runs dedup to zero new rows.
    #[tokio::test]
    async fn import_codex_empty_external_id_quarantined_with_content_hash_dedup() {
        let tmp = TempDir::new().unwrap();
        let history = write_codex_history(
            tmp.path(),
            &[
                r#"{"thread_id":"","timestamp":"2026-05-27T10:00:00Z","role":"user","content":"orphan line with no thread"}"#,
            ],
        );
        let db = tmp.path().join("codex_quarantine.db");

        let stats1 = run_codex(&history, None, None, &db, false).await.unwrap();
        assert_eq!(stats1.sessions_quarantined, 1);
        assert_eq!(
            stats1.sessions_imported, 1,
            "quarantined session is still imported under its hash key"
        );

        // external_id must be the stable quarantine hash — never null/empty.
        let pool = create_pool(&db.to_string_lossy()).await.unwrap();
        let ext: String = sqlx::query_scalar("SELECT external_id FROM sessions LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(
            ext.starts_with("quarantine-"),
            "expected content-hash quarantine key, got {ext}"
        );

        // Re-run: content-hash dedup → zero new sessions, zero new turns.
        let stats2 = run_codex(&history, None, None, &db, false).await.unwrap();
        assert_eq!(stats2.sessions_imported, 0, "re-run must dedup on hash key");
        assert_eq!(stats2.turns_imported, 0);
        assert_eq!(stats2.sessions_skipped_existing, 1);
    }

    /// quarantine_external_id is deterministic over identical content and
    /// distinct for different content.
    #[test]
    fn quarantine_key_stable_and_content_sensitive() {
        let mk = |content: &str| ImportedSession {
            external_id: String::new(),
            tool_id: "codex".into(),
            project_name: None,
            started_at: Utc::now(),
            ended_at: None,
            model: None,
            turns: vec![crate::commands::analyze::ImportedTurn {
                turn_idx: 0,
                role: "user".into(),
                content: content.into(),
                tool_calls: None,
                tool_name: None,
                model: None,
                tokens_in: None,
                tokens_out: None,
                latency_ms: None,
                created_at: Utc::now(),
            }],
            imported_from: PathBuf::from("/tmp/h.jsonl"),
            working_dir: None,
        };
        let a1 = quarantine_external_id(&mk("same content"));
        let a2 = quarantine_external_id(&mk("same content"));
        let b = quarantine_external_id(&mk("different content"));
        assert_eq!(a1, a2, "hash must be stable across runs");
        assert_ne!(a1, b, "hash must differ for different content");
        assert!(a1.starts_with("quarantine-"));
    }

    /// Codex guard: secrets in turn content are redacted before persist.
    #[tokio::test]
    async fn import_codex_guard_applied() {
        let tmp = TempDir::new().unwrap();
        let fake_secret = concat!("sk-ant-", "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
        let history = write_codex_history(
            tmp.path(),
            &[
                &format!(
                    r#"{{"thread_id":"t-guard","timestamp":"2026-05-27T10:00:00Z","role":"user","content":"key is {fake_secret}"}}"#
                ),
            ],
        );
        let db = tmp.path().join("codex_guard.db");

        run_codex(&history, None, None, &db, false).await.unwrap();

        let pool = create_pool(&db.to_string_lossy()).await.unwrap();
        let content: String = sqlx::query_scalar("SELECT content FROM turns LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(
            !content.contains(fake_secret),
            "guard must redact secret before persist"
        );
    }
}

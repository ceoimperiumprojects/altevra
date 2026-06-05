//! `altevra import` — tool-native session backfill.
//!
//! Imports historical sessions from a supported AI tool into Altevra's
//! omniscient recorder. Currently handles `--tool hermes`, which reads the
//! `YYYYMMDD_HHMMSS_<hex>.jsonl` files Hermes writes under
//! `~/.hermes/sessions/`. The Hermes directory is **read-only** for Altevra
//! — we never write to Pavle's actual session history.
//!
//! Design notes (parity with [`analyze`]):
//!
//! * The parser ([`parsers::hermes::parse_session_jsonl`]) does the schema
//!   work; this command is the orchestrator: discover → filter → upsert →
//!   guard → record_turn → enqueue improvement signal.
//! * Idempotency: `SessionsRepository::upsert_imported` keys on
//!   `(tool, external_id)`, so re-running the command produces 0 new
//!   sessions/turns. Skipped sessions never enqueue a second signal.
//! * Pre-write safety (R11 / SI-7): every turn `content` and every JSON
//!   leaf inside `tool_calls` is run through `guard_text` / `guard_json`
//!   BEFORE persistence, with sensitivity + redaction status combined and
//!   stored on the row.
//! * Improvement signal (C1): after each successfully-imported session we
//!   call `signal_for_session` and best-effort insert one row — same
//!   policy as the live hook handler, so the orchestrator can pick the
//!   backfilled sessions up later. Resident-mode sessions are skipped via
//!   SI-6 inside the signal producer.
//!
//! [`analyze`]: super::analyze

use altevra_db::{
    create_pool, run_migrations, signal_for_session, ImprovementSignalsRepository, SessionRow,
    SessionsRepository, TurnRow,
};
use chrono::{DateTime, NaiveDate, Utc};
use clap::Args;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::commands::analyze::parsers;
use crate::commands::analyze::ImportedSession;

#[derive(Args, Debug)]
pub struct ImportArgs {
    /// Which tool to import from. Currently supported: `hermes`. The flag is
    /// intentionally an enum-shaped string so future tools (codex, cursor,
    /// claude-code) drop in as additional match arms.
    #[arg(long)]
    pub tool: String,

    /// Lower bound on session start time. Accepts RFC3339 (`2026-05-01T00:00:00Z`)
    /// or a bare date (`2026-05-01`, interpreted as midnight UTC). Sessions
    /// older than this are skipped without reading the file body — the
    /// filename timestamp is enough.
    #[arg(long)]
    pub since: Option<String>,

    /// SQLite database path. Defaults to the Altevra workspace DB.
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,

    /// Discover + plan only. No writes to the database, no signal enqueues.
    #[arg(long)]
    pub dry_run: bool,

    /// Override the source directory. Defaults to `$HOME/.hermes/sessions`
    /// for `--tool hermes`. Used by tests to point at a fixture tree.
    #[arg(long)]
    pub source_dir: Option<PathBuf>,
}

/// Compact in-process counters reported at the end of the run.
#[derive(Debug, Default, Clone)]
pub struct ImportStats {
    pub discovered: usize,
    pub filtered_out: usize,
    pub sessions_imported: u64,
    pub sessions_skipped_existing: u64,
    pub sessions_skipped_empty: u64,
    pub turns_imported: u64,
    pub signals_enqueued: u64,
    pub errors: Vec<String>,
}

pub async fn run(args: ImportArgs) -> anyhow::Result<()> {
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
        other => anyhow::bail!(
            "unsupported --tool {other}; supported: hermes (claude-code/codex/cursor land in v0.5)"
        ),
    }
}

fn default_hermes_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".hermes/sessions")
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
            Err(e) => stats
                .errors
                .push(format!("parse {}: {e}", path.display())),
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

    for turn in &sess.turns {
        // R11 / SI-7: guard BEFORE persist. Same pipeline the live hook
        // handler uses — secrets + PII are scrubbed, sensitivity bumped,
        // tool_calls JSON walked leaf-by-leaf via guard_json.
        let guarded = altevra_secrets::guard_text(
            &turn.content,
            altevra_core::Sensitivity::Internal,
        );
        let mut sensitivity = guarded.sensitivity.clone();
        let mut redaction = guarded.redaction_status.clone();
        let mut redacted_count = guarded.sightings.len() as i64
            + i64::from(
                guarded
                    .risk_tags
                    .contains(&altevra_core::RiskTag::ThirdPartyPii),
            );

        let scrubbed_tool_calls = if let Some(tc) = turn.tool_calls.as_ref() {
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

    Ok(())
}

fn print_report(stats: &ImportStats, dry_run: bool) {
    println!();
    if dry_run {
        println!(
            "[dry-run] would process {} sessions ({} filtered out by --since)",
            stats.discovered - stats.filtered_out,
            stats.filtered_out
        );
        return;
    }
    println!(
        "imported {} new sessions, {} new turns, {} new signals; skipped {}",
        stats.sessions_imported,
        stats.turns_imported,
        stats.signals_enqueued,
        stats.sessions_skipped_existing + stats.sessions_skipped_empty,
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
}

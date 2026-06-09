//! `altevra db` — database maintenance: `unify` + `replay-spool` (PLAN-ALIVE §P0.2).
//!
//! ## `altevra db unify`
//!
//! Merges every discovered shadow `altevra.db` (CWD-relative spawns from the
//! pre-S0 path bug) into the ONE canonical DB at `~/.altevra/altevra.db`.
//! Hard rules, all mechanisms (not assertions):
//!
//! * **Refuses if the brain PID is alive**; takes the TTL'd maintenance lock
//!   ([`altevra_core::maintenance`]) so hooks spool and batch writers refuse.
//! * **Shadow 033→034 upgrade by introspection** — `PRAGMA table_info` +
//!   targeted `ALTER TABLE … ADD COLUMN working_dir TEXT`, NEVER the sqlx
//!   migrator on a foreign DB (checksum drift + over-application risk).
//! * **Backup is checkpoint-then-copy:** `PRAGMA wal_checkpoint(TRUNCATE)`
//!   per DB, then copy `db` + `-wal` + `-shm` to `~/.altevra/backups/<ts>/`.
//! * **The merge transaction writes ONLY the canonical DB.** Shadows are
//!   ATTACHed read-only-by-discipline (every statement writes `main.*`);
//!   quarantine is a filesystem rename AFTER commit, never an in-txn write.
//! * **Conservative dedup (locked):** non-null `external_id` keys on
//!   `(tool, external_id)`; NULL-external_id sessions auto-merge ONLY on
//!   session-id match OR full ordered NON-EMPTY turn-sequence hash; partial /
//!   ambiguous matches are quarantined (left in the shadow + conflict report).
//!   Turns collapse only on a full `(session_id, turn_idx, role, content_hash,
//!   tool_calls_hash, file_changes_hash)` match; divergent `(session_id,
//!   turn_idx)` collisions land in `turns_quarantine` (migration 035).
//! * **FK remap set (explicit):** `turns.session_id`,
//!   `file_changes.session_id/turn_id`, `improvement_signals.source_ref`
//!   payload refs, `proposals.evidence_refs`, `events.entity_id`.
//! * **FTS is app-maintained:** merged objects are explicitly re-indexed via
//!   [`FtsRepository::index`] after commit (no triggers exist in this schema).
//! * **Merged shadow turns are re-guarded** (guard_text/guard_json) — 033-era
//!   rows predate current redaction hardening; anything that cannot keep a
//!   clean/redacted verdict is marked `unscanned` (ExposureGate fail-closes).
//! * `--dry-run` runs the FULL merge against temp copies of the shadows and
//!   ROLLS BACK, printing exact before/after per-table counts + the conflict
//!   report. A real run REQUIRES `--apply` (belt and suspenders).
//!
//! ## `altevra db replay-spool`
//!
//! Mandatory idempotent unify epilogue. Drains `~/.altevra/state/spool/` via
//! **direct-by-id ingest that errors loudly** — NEVER through the hook
//! pointer-lookup path (the pointer may be gone by replay time → silent data
//! loss). Failure keeps the file + writes an `audit_log` row; success removes
//! the file; re-runs are idempotent (spooled turn ids are stable).

use altevra_core::maintenance::{
    maintenance_lock_path, maintenance_locked_default, spool_dir, MaintenanceLock,
};
use altevra_db::{create_pool, run_migrations, FtsRepository, SessionRow, SessionsRepository, TurnRow};
use altevra_secrets::guard_text;
use chrono::{DateTime, Utc};
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqliteConnection, SqlitePool};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::commands::hook_handle::guard_json;

#[derive(Subcommand)]
pub enum DbCommands {
    /// Merge every discovered shadow altevra.db into the canonical DB.
    /// Preview with --dry-run; a real run requires --apply.
    Unify(UnifyArgs),
    /// Drain the hook spool (~/.altevra/state/spool) into the canonical DB
    /// via direct-by-id ingest. Idempotent; mandatory after a real unify.
    ReplaySpool(ReplaySpoolArgs),
}

#[derive(Args)]
pub struct UnifyArgs {
    /// Canonical SQLite database path (the merge TARGET).
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,

    /// Explicit shadow DB path(s) to merge (repeatable).
    #[arg(long)]
    pub shadow: Vec<PathBuf>,

    /// Root(s) to scan for `<dir>/.altevra/altevra.db` shadows (repeatable).
    /// When neither --shadow nor --scan-root is given, scans $HOME/projekti
    /// and $HOME/Desktop plus the current working directory.
    #[arg(long)]
    pub scan_root: Vec<PathBuf>,

    /// Preview: run the full merge against temp copies and roll back.
    /// Prints exact before/after per-table counts + the conflict report.
    #[arg(long)]
    pub dry_run: bool,

    /// Actually perform the merge (backup → merge → quarantine → replay-spool).
    #[arg(long)]
    pub apply: bool,

    /// Backup directory root (default: ~/.altevra/backups).
    #[arg(long)]
    pub backup_dir: Option<PathBuf>,

    /// Brain PID file checked for liveness (unify refuses while brain runs).
    #[arg(long, default_value_os_t = altevra_core::default_brain_pid_path())]
    pub brain_pid: PathBuf,
}

#[derive(Args)]
pub struct ReplaySpoolArgs {
    /// Canonical SQLite database path.
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,

    /// Spool directory override (default: ~/.altevra/state/spool).
    #[arg(long)]
    pub spool_dir: Option<PathBuf>,
}

pub async fn run(cmd: DbCommands) -> anyhow::Result<()> {
    match cmd {
        DbCommands::Unify(args) => run_unify(args).await,
        DbCommands::ReplaySpool(args) => run_replay(args).await,
    }
}

// ===========================================================================
// CLI entrypoints
// ===========================================================================

async fn run_unify(args: UnifyArgs) -> anyhow::Result<()> {
    if args.dry_run == args.apply {
        anyhow::bail!(
            "pass exactly one of --dry-run (preview, no writes) or --apply \
             (real merge). Nothing was changed."
        );
    }

    let canonical = args.db.clone();
    let shadows = discover_shadows(&canonical, &args.shadow, &args.scan_root);
    if shadows.is_empty() {
        println!("No shadow databases discovered — nothing to unify.");
        return Ok(());
    }
    println!("Canonical: {}", canonical.display());
    for s in &shadows {
        println!("Shadow:    {}", s.display());
    }

    if let Some(pid) = brain_pid_alive(&args.brain_pid) {
        if args.apply {
            anyhow::bail!(
                "brain daemon is alive (PID {pid}, {}). Stop it first: `altevra brain stop`",
                args.brain_pid.display()
            );
        }
        eprintln!("[altevra] warning: brain is alive (PID {pid}); dry-run counts may drift.");
    }

    let backup_root = args
        .backup_dir
        .clone()
        .unwrap_or_else(|| altevra_core::home_dir().join(".altevra/backups"));

    let opts = UnifyOptions {
        canonical: canonical.clone(),
        shadows,
        apply: args.apply,
        backup_root,
    };

    let report = if args.apply {
        // The maintenance lock makes hooks spool + batch writers refuse for
        // the whole merge window. Released BEFORE the spool replay epilogue.
        let lock = MaintenanceLock::acquire(&maintenance_lock_path(), "db unify")?;
        let result = unify(&opts).await;
        lock.release()?;
        result?
    } else {
        unify(&opts).await?
    };

    print_report(&report);

    if args.apply {
        // Mandatory idempotent epilogue: drain whatever the hooks spooled
        // while the lock was held. Runs AFTER lock release so the writes land
        // in the (now unified) canonical DB.
        let pool = create_pool(&canonical.to_string_lossy()).await?;
        run_migrations(&pool).await?;
        let replay = replay_spool_dir(&pool, &spool_dir()).await?;
        println!(
            "Spool replay: {} replayed, {} failed.",
            replay.replayed, replay.failed
        );
        if replay.failed > 0 {
            anyhow::bail!(
                "{} spool file(s) failed to replay (kept on disk, audit_log rows \
                 written) — inspect {} and re-run `altevra db replay-spool`",
                replay.failed,
                spool_dir().display()
            );
        }
    }
    Ok(())
}

async fn run_replay(args: ReplaySpoolArgs) -> anyhow::Result<()> {
    if maintenance_locked_default() {
        anyhow::bail!(
            "maintenance lock is held (db unify in progress) — replay refused; \
             retry after unify completes"
        );
    }
    let dir = args.spool_dir.clone().unwrap_or_else(spool_dir);
    let pool = create_pool(&args.db.to_string_lossy()).await?;
    run_migrations(&pool).await?;
    let report = replay_spool_dir(&pool, &dir).await?;
    println!(
        "Spool replay: {} replayed, {} failed.",
        report.replayed, report.failed
    );
    if report.failed > 0 {
        anyhow::bail!(
            "{} spool file(s) failed to replay — files kept, audit_log rows written",
            report.failed
        );
    }
    Ok(())
}

// ===========================================================================
// Spool protocol (written by hook_handle under maintenance lock, drained here)
// ===========================================================================

/// One spooled hook event. The payload is guard-redacted BEFORE it reaches
/// disk (never raw secrets/PII in the spool). Turn entries embed the
/// session_id + the FULL turn payload + a pre-generated stable turn id so
/// replay is direct-by-id and idempotent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum SpoolEntry {
    SessionStart {
        tool: String,
        session_id: Uuid,
        project_name: Option<String>,
        started_at: DateTime<Utc>,
        working_dir: Option<String>,
    },
    SessionEnd {
        tool: String,
        session_id: Uuid,
        /// Already guard_text-redacted.
        summary: Option<String>,
        ended_at: DateTime<Utc>,
    },
    Turn {
        tool: String,
        session_id: Uuid,
        /// Stable id minted at spool time → replay re-runs are idempotent.
        turn_id: Uuid,
        role: String,
        /// guard_text-redacted before disk.
        content: String,
        tool_name: Option<String>,
        /// guard_json-redacted before disk.
        tool_calls: Option<serde_json::Value>,
        /// guard_json-redacted before disk.
        file_changes: Option<serde_json::Value>,
        model: Option<String>,
        tokens_in: Option<i64>,
        tokens_out: Option<i64>,
        latency_ms: Option<i64>,
        redacted_count: i64,
        sensitivity: String,
        redaction_status: String,
        working_dir: Option<String>,
        created_at: DateTime<Utc>,
    },
}

impl SpoolEntry {
    fn ts(&self) -> DateTime<Utc> {
        match self {
            Self::SessionStart { started_at, .. } => *started_at,
            Self::SessionEnd { ended_at, .. } => *ended_at,
            Self::Turn { created_at, .. } => *created_at,
        }
    }
}

/// Build a guarded Turn spool entry from a raw hook payload. Mirrors the live
/// `record_turn` mapping (role/content/tool_name per event) minus auto_capture
/// (acknowledged degradation: spooled content is pre-redacted, so auto_capture
/// never sees raw values for spooled turns). Returns `None` for non-turn events.
pub(crate) fn build_spool_turn(
    tool: &str,
    session_id: Uuid,
    event: &str,
    payload: &serde_json::Value,
    working_dir: Option<String>,
) -> Option<SpoolEntry> {
    use altevra_core::status::RedactionStatus;
    use altevra_core::Sensitivity;

    let as_str = |keys: &[&str]| -> Option<String> {
        keys.iter()
            .find_map(|k| payload.get(*k).and_then(serde_json::Value::as_str))
            .map(String::from)
    };
    let tool_name = payload
        .get("tool_name")
        .and_then(serde_json::Value::as_str)
        .map(String::from);

    let (role, raw_content) = match event {
        "user_prompt_submit" => (
            "user",
            as_str(&["user_prompt", "prompt", "content", "message"]).unwrap_or_default(),
        ),
        "pre_tool_use" => (
            "tool_call",
            payload
                .get("tool_input")
                .map(|v| v.to_string())
                .unwrap_or_default(),
        ),
        "post_tool_use" => (
            "tool_result",
            as_str(&["tool_response", "result", "output"]).unwrap_or_default(),
        ),
        _ => return None,
    };

    // Guard EVERYTHING before it can reach disk.
    let guarded = guard_text(&raw_content, Sensitivity::Internal);
    let mut redacted_count = guarded.sightings.len() as i64;
    if guarded
        .risk_tags
        .contains(&altevra_core::RiskTag::ThirdPartyPii)
    {
        redacted_count += 1;
    }
    let mut sensitivity = guarded.sensitivity.clone();
    let mut redaction = guarded.redaction_status.clone();

    let mut guard_side = |v: Option<&serde_json::Value>| -> Option<serde_json::Value> {
        let v = v?;
        let (scrubbed, n, sens) = guard_json(v);
        redacted_count += n;
        sensitivity = sensitivity.combine(&sens);
        if n > 0 {
            redaction = RedactionStatus::Redacted;
        }
        Some(scrubbed)
    };
    let tool_calls = guard_side(payload.get("tool_input"));
    let file_changes = guard_side(payload.get("file_changes"));

    Some(SpoolEntry::Turn {
        tool: tool.to_string(),
        session_id,
        turn_id: Uuid::new_v4(),
        role: role.to_string(),
        content: guarded.value,
        tool_name,
        tool_calls,
        file_changes,
        model: payload
            .get("model")
            .and_then(serde_json::Value::as_str)
            .map(String::from),
        tokens_in: payload.get("tokens_in").and_then(serde_json::Value::as_i64),
        tokens_out: payload
            .get("tokens_out")
            .and_then(serde_json::Value::as_i64),
        latency_ms: payload
            .get("latency_ms")
            .and_then(serde_json::Value::as_i64),
        redacted_count,
        sensitivity: sensitivity.to_string(),
        redaction_status: redaction.to_string(),
        working_dir,
        created_at: Utc::now(),
    })
}

/// Write ONE spool entry to `<dir>/<tool>-<pid>-<ts>.json` — O_EXCL (atomic
/// create-new), mode 0600 at open. The entry must already be guard-redacted.
pub(crate) fn write_spool_entry(
    dir: &Path,
    tool: &str,
    entry: &SpoolEntry,
) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let tool_safe: String = tool
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let pid = std::process::id();
    let mut nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let body = serde_json::to_vec_pretty(entry)?;

    // Same pid fires many events — bump the timestamp on O_EXCL collision.
    for _ in 0..64 {
        let path = dir.join(format!("{tool_safe}-{pid}-{nanos}.json"));
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        match opts.open(&path) {
            Ok(mut f) => {
                use std::io::Write;
                f.write_all(&body)?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                nanos += 1;
                continue;
            }
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::bail!("could not allocate a unique spool file name in {}", dir.display())
}

#[derive(Debug, Default)]
pub(crate) struct ReplayReport {
    pub replayed: usize,
    pub failed: usize,
}

/// Drain the spool directory into the canonical DB. Direct-by-id ingest:
/// every entry carries its session_id (and turn id); a missing session is a
/// LOUD error (file kept + audit_log row), never a silent skip. Successful
/// entries remove their file; re-runs are idempotent.
pub(crate) async fn replay_spool_dir(
    pool: &SqlitePool,
    dir: &Path,
) -> anyhow::Result<ReplayReport> {
    let mut report = ReplayReport::default();
    if !dir.exists() {
        return Ok(report);
    }

    // Parse everything first so we can replay in event-time order across
    // tools/pids (filename order alone interleaves tools incorrectly).
    let mut entries: Vec<(PathBuf, SpoolEntry)> = Vec::new();
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
        .collect();
    paths.sort();
    for path in paths {
        match std::fs::read_to_string(&path)
            .map_err(anyhow::Error::from)
            .and_then(|s| serde_json::from_str::<SpoolEntry>(&s).map_err(anyhow::Error::from))
        {
            Ok(entry) => entries.push((path, entry)),
            Err(e) => {
                eprintln!(
                    "[altevra] spool replay: cannot parse {} — keeping file: {e}",
                    path.display()
                );
                audit_spool_failure(pool, &path, &format!("parse error: {e}")).await;
                report.failed += 1;
            }
        }
    }
    entries.sort_by_key(|(p, e)| (e.ts(), p.clone()));

    for (path, entry) in entries {
        match replay_entry(pool, &entry).await {
            Ok(()) => {
                if let Err(e) = std::fs::remove_file(&path) {
                    eprintln!(
                        "[altevra] spool replay: ingested but could not remove {} — \
                         replay is idempotent, safe to retry: {e}",
                        path.display()
                    );
                }
                report.replayed += 1;
            }
            Err(e) => {
                eprintln!(
                    "[altevra] spool replay FAILED for {} (file kept): {e:#}",
                    path.display()
                );
                audit_spool_failure(pool, &path, &format!("{e:#}")).await;
                report.failed += 1;
            }
        }
    }
    Ok(report)
}

async fn replay_entry(pool: &SqlitePool, entry: &SpoolEntry) -> anyhow::Result<()> {
    let repo = SessionsRepository::new(pool);
    match entry {
        SpoolEntry::SessionStart {
            tool,
            session_id,
            project_name,
            started_at,
            working_dir,
        } => {
            if session_exists(pool, session_id).await? {
                return Ok(()); // idempotent re-run
            }
            repo.start_session(&SessionRow {
                id: *session_id,
                tool: tool.clone(),
                project_id: None,
                project_name: project_name.clone(),
                started_at: *started_at,
                ended_at: None,
                summary: None,
                tokens_in_total: 0,
                tokens_out_total: 0,
                cost_usd_estimate: 0.0,
                turn_count: 0,
                metadata: serde_json::json!({"started_via": "hook", "spooled": true}),
                external_id: None,
                imported_from: None,
                working_dir: working_dir.clone(),
            })
            .await
        }
        SpoolEntry::SessionEnd {
            session_id,
            summary,
            ended_at,
            ..
        } => {
            if !session_exists(pool, session_id).await? {
                anyhow::bail!(
                    "spooled session_end references unknown session {session_id} — \
                     refusing silent drop"
                );
            }
            sqlx::query(
                "UPDATE sessions SET ended_at = COALESCE(ended_at, ?), \
                 summary = COALESCE(summary, ?) WHERE id = ?",
            )
            .bind(ended_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
            .bind(summary.as_deref())
            .bind(session_id.to_string())
            .execute(pool)
            .await?;
            Ok(())
        }
        SpoolEntry::Turn {
            tool,
            session_id,
            turn_id,
            role,
            content,
            tool_name,
            tool_calls,
            file_changes,
            model,
            tokens_in,
            tokens_out,
            latency_ms,
            redacted_count,
            sensitivity,
            redaction_status,
            working_dir,
            created_at,
        } => {
            // Idempotency: the spooled turn id is stable; if it already landed
            // (prior replay that failed at file-removal), this is a no-op.
            let already: Option<i64> = sqlx::query_scalar("SELECT 1 FROM turns WHERE id = ?")
                .bind(turn_id.to_string())
                .fetch_optional(pool)
                .await?;
            if already.is_some() {
                return Ok(());
            }
            if !session_exists(pool, session_id).await? {
                anyhow::bail!(
                    "spooled turn {turn_id} references unknown session {session_id} — \
                     refusing silent drop (was the session created before the lock?)"
                );
            }
            let turn_idx = repo.next_turn_idx(*session_id).await?;
            repo.record_turn(&TurnRow {
                id: *turn_id,
                session_id: *session_id,
                turn_idx,
                role: role.clone(),
                content: content.clone(),
                tool_calls: tool_calls.clone(),
                tool_name: tool_name.clone(),
                model: model.clone(),
                tokens_in: *tokens_in,
                tokens_out: *tokens_out,
                latency_ms: *latency_ms,
                file_changes: file_changes.clone(),
                redacted_count: *redacted_count,
                source_tool: Some(tool.clone()),
                sensitivity: sensitivity.clone(),
                redaction_status: redaction_status.clone(),
                created_at: *created_at,
                working_dir: working_dir.clone(),
            })
            .await
        }
    }
}

async fn session_exists(pool: &SqlitePool, id: &Uuid) -> anyhow::Result<bool> {
    let row: Option<i64> = sqlx::query_scalar("SELECT 1 FROM sessions WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
}

async fn audit_spool_failure(pool: &SqlitePool, file: &Path, err: &str) {
    let mut msg = err.to_string();
    msg.truncate(500);
    let details = serde_json::json!({ "error": msg }).to_string();
    if let Err(e) = sqlx::query(
        "INSERT INTO audit_log (id, action, subject_type, subject_id, actor, details) \
         VALUES (?, 'spool_replay_failed', 'spool_file', ?, 'system', ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(file.file_name().and_then(|n| n.to_str()).unwrap_or("?"))
    .bind(details)
    .execute(pool)
    .await
    {
        eprintln!("[altevra] audit_log write failed (non-fatal): {e}");
    }
}

// ===========================================================================
// Shadow discovery + preconditions
// ===========================================================================

/// Discover shadow DBs: explicit `--shadow` paths, `<cwd>/.altevra/altevra.db`,
/// and any `<dir>/.altevra/altevra.db` under the scan roots (depth-limited,
/// heavy dirs skipped). The canonical DB itself is always excluded.
fn discover_shadows(canonical: &Path, explicit: &[PathBuf], scan_roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    for p in explicit {
        if p.exists() {
            candidates.push(p.clone());
        } else {
            eprintln!("[altevra] --shadow {} does not exist — skipped", p.display());
        }
    }

    let mut roots: Vec<PathBuf> = scan_roots.to_vec();
    if explicit.is_empty() && scan_roots.is_empty() {
        // Default known locations: CWD + the project trees where the
        // CWD-relative bug historically spawned shadows.
        if let Ok(cwd) = std::env::current_dir() {
            let c = cwd.join(".altevra/altevra.db");
            if c.exists() {
                candidates.push(c);
            }
        }
        for d in ["projekti", "Desktop"] {
            let r = altevra_core::home_dir().join(d);
            if r.is_dir() {
                roots.push(r);
            }
        }
    }

    const SKIP_DIRS: &[&str] = &[
        "node_modules", "target", ".git", ".cache", ".venv", "venv", "dist", "build",
        ".cargo", ".rustup",
    ];
    for root in &roots {
        let walker = walkdir::WalkDir::new(root)
            .max_depth(6)
            .into_iter()
            .filter_entry(|e| {
                let name = e.file_name().to_string_lossy();
                !(e.file_type().is_dir() && SKIP_DIRS.contains(&name.as_ref()))
            });
        for entry in walker.flatten() {
            if entry.file_type().is_file()
                && entry.file_name() == "altevra.db"
                && entry
                    .path()
                    .parent()
                    .and_then(|p| p.file_name())
                    .map(|n| n == ".altevra")
                    .unwrap_or(false)
            {
                candidates.push(entry.path().to_path_buf());
            }
        }
    }

    // Canonicalize, dedupe, drop the canonical DB itself.
    let canon_real = canonical.canonicalize().unwrap_or_else(|_| canonical.to_path_buf());
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut out = Vec::new();
    for c in candidates {
        let real = c.canonicalize().unwrap_or_else(|_| c.clone());
        if real == canon_real {
            continue;
        }
        if seen.insert(real.clone()) {
            out.push(real);
        }
    }
    out.sort();
    out
}

/// Returns the brain PID when the PID file points at a live process.
fn brain_pid_alive(pid_file: &Path) -> Option<i32> {
    let pid: i32 = std::fs::read_to_string(pid_file).ok()?.trim().parse().ok()?;
    if pid <= 0 {
        return None;
    }
    #[cfg(unix)]
    {
        // kill(pid, 0): 0 → alive (or EPERM, also alive). ESRCH → dead.
        let alive = unsafe { libc::kill(pid, 0) } == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
        if alive {
            return Some(pid);
        }
    }
    None
}

// ===========================================================================
// Unify engine
// ===========================================================================

pub(crate) struct UnifyOptions {
    pub canonical: PathBuf,
    pub shadows: Vec<PathBuf>,
    pub apply: bool,
    pub backup_root: PathBuf,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct ContentMergeCounts {
    pub inserted: u64,
    pub identical: i64,
    pub id_conflicts: i64,
    pub unique_collisions: i64,
}

#[derive(Debug, Default)]
pub(crate) struct UnifyReport {
    pub applied: bool,
    pub shadows: Vec<String>,
    pub counts_before: BTreeMap<String, i64>,
    pub counts_after: BTreeMap<String, i64>,
    pub sessions_new: usize,
    pub sessions_merged: usize,
    pub sessions_quarantined: usize,
    pub turns_inserted: usize,
    pub turns_collapsed: usize,
    pub turns_quarantined: usize,
    pub file_changes_inserted: usize,
    pub file_changes_skipped: usize,
    pub events_inserted: usize,
    pub events_skipped: usize,
    pub signals_inserted: usize,
    pub signals_skipped: usize,
    pub proposals_inserted: usize,
    pub proposals_skipped: usize,
    pub content: BTreeMap<String, ContentMergeCounts>,
    pub fts_reindexed: usize,
    pub conflicts: Vec<String>,
    pub backups: Vec<PathBuf>,
    pub quarantined_paths: Vec<PathBuf>,
}

/// Tables reported in the before/after count table.
const COUNT_TABLES: &[&str] = &[
    "sessions",
    "turns",
    "turns_quarantine",
    "file_changes",
    "events",
    "improvement_signals",
    "proposals",
    "object_index",
    "object_fts",
    "learnings",
    "wiki_pages",
    "relations",
    "research_items",
];

/// Content tables merged generically by primary id (beyond sessions/turns/
/// file_changes/events/signals/proposals which need Rust-side remap logic).
/// `(table, pk columns, extra unique-key guard SQL over `s`/`m2`)`.
const CONTENT_TABLES: &[(&str, &[&str], Option<&str>)] = &[
    ("object_index", &["type", "id"], None),
    ("learnings", &["id"], None),
    (
        "wiki_pages",
        &["id"],
        Some("EXISTS (SELECT 1 FROM main.wiki_pages m2 WHERE m2.topic = s.topic)"),
    ),
    (
        "relations",
        &["id"],
        Some(
            "EXISTS (SELECT 1 FROM main.relations m2 WHERE m2.from_type = s.from_type \
             AND m2.from_id = s.from_id AND m2.rel = s.rel AND m2.to_type IS s.to_type \
             AND m2.to_id IS s.to_id AND m2.to_ref IS s.to_ref)",
        ),
    ),
    (
        "research_items",
        &["id"],
        Some(
            "EXISTS (SELECT 1 FROM main.research_items m2 WHERE m2.feed_id = s.feed_id \
             AND m2.guid = s.guid)",
        ),
    ),
];

/// The full unify pass. Caller holds the maintenance lock for `apply` runs.
/// Dry-run operates on temp COPIES of the shadows and rolls the merge
/// transaction back — originals are never touched.
pub(crate) async fn unify(opts: &UnifyOptions) -> anyhow::Result<UnifyReport> {
    let mut report = UnifyReport {
        applied: opts.apply,
        ..Default::default()
    };

    // ---- 1. Backups (apply only): checkpoint-then-copy each DB ----
    let backup_dir = opts
        .backup_root
        .join(Utc::now().format("%Y%m%d-%H%M%S").to_string());
    if opts.apply {
        let mut manifest: Vec<serde_json::Value> = Vec::new();
        for (i, db) in std::iter::once(&opts.canonical)
            .chain(opts.shadows.iter())
            .enumerate()
        {
            let label = if i == 0 { "canonical".to_string() } else { format!("shadow{i}") };
            let copies = checkpoint_and_backup(db, &backup_dir, &label).await?;
            manifest.push(serde_json::json!({
                "label": label,
                "source": db.to_string_lossy(),
                "copies": copies.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>(),
            }));
            report.backups.extend(copies);
        }
        std::fs::write(
            backup_dir.join("manifest.json"),
            serde_json::to_vec_pretty(&serde_json::json!({ "backed_up": manifest }))?,
        )?;
    }

    // ---- 2. Working set: apply → originals; dry-run → temp copies ----
    let _tmp_holder; // keeps dry-run copies alive until we are done
    let working_shadows: Vec<PathBuf> = if opts.apply {
        report.shadows = opts
            .shadows
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        opts.shadows.clone()
    } else {
        let tmp = tempfile::TempDir::new()?;
        let mut copies = Vec::new();
        for (i, s) in opts.shadows.iter().enumerate() {
            let dst = tmp.path().join(format!("shadow{i}.db"));
            std::fs::copy(s, &dst)?;
            for ext in ["-wal", "-shm"] {
                let side = sibling(s, ext);
                if side.exists() {
                    std::fs::copy(&side, sibling(&dst, ext))?;
                }
            }
            report.shadows.push(s.to_string_lossy().into_owned());
            copies.push(dst);
        }
        _tmp_holder = tmp;
        copies
    };

    // ---- 3. Canonical pool + the single merge connection ----
    let pool = create_pool(&opts.canonical.to_string_lossy()).await?;
    run_migrations(&pool).await?;
    let mut conn = pool.acquire().await?;

    // ATTACH every shadow (read by discipline: every write targets main.*).
    for (i, s) in working_shadows.iter().enumerate() {
        let escaped = s.to_string_lossy().replace('\'', "''");
        sqlx::query(&format!("ATTACH DATABASE '{escaped}' AS sh{i}"))
            .execute(&mut *conn)
            .await?;
        // Shadow 033→034 upgrade by INTROSPECTION (never the sqlx migrator on
        // a foreign DB). Outside the merge txn: in apply mode the original is
        // already backed up; in dry-run this mutates only the temp copy.
        upgrade_shadow_schema(&mut conn, &format!("sh{i}")).await?;
    }

    // ---- 4. ONE canonical-only merge transaction ----
    sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;
    let merge_result = merge_all(&mut conn, working_shadows.len(), &opts.shadows, &mut report).await;
    let fts_rows = match merge_result {
        Ok(rows) => {
            if opts.apply {
                sqlx::query("COMMIT").execute(&mut *conn).await?;
            } else {
                sqlx::query("ROLLBACK").execute(&mut *conn).await?;
            }
            rows
        }
        Err(e) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
            for i in 0..working_shadows.len() {
                let _ = sqlx::query(&format!("DETACH DATABASE sh{i}"))
                    .execute(&mut *conn)
                    .await;
            }
            return Err(e);
        }
    };

    for i in 0..working_shadows.len() {
        sqlx::query(&format!("DETACH DATABASE sh{i}"))
            .execute(&mut *conn)
            .await?;
    }
    drop(conn);

    // ---- 5. FTS is APP-MAINTAINED: re-index merged objects explicitly ----
    report.fts_reindexed = fts_rows.len();
    if opts.apply {
        let fts = FtsRepository::new(&pool);
        for r in &fts_rows {
            fts.index(&r.object_type, &r.object_id, &r.title, &r.body, &r.tags)
                .await?;
        }
        // Recount object_fts after the post-commit reindex.
        if let Ok(n) =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM object_fts").fetch_one(&pool).await
        {
            report.counts_after.insert("object_fts".into(), n);
        }
    }
    pool.close().await;

    // ---- 6. Quarantine (rename, never delete) AFTER commit ----
    if opts.apply {
        let ts = Utc::now().format("%Y%m%d-%H%M%S").to_string();
        for s in &opts.shadows {
            let target = sibling(s, &format!(".quarantined-{ts}"));
            std::fs::rename(s, &target)?;
            for ext in ["-wal", "-shm"] {
                let side = sibling(s, ext);
                if side.exists() {
                    let _ = std::fs::rename(&side, sibling(&target, ext));
                }
            }
            report.quarantined_paths.push(target);
        }
    }

    Ok(report)
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut os = path.as_os_str().to_owned();
    os.push(suffix);
    PathBuf::from(os)
}

/// `PRAGMA wal_checkpoint(TRUNCATE)` then copy db + -wal + -shm into the
/// backup dir. Returns the created copies.
async fn checkpoint_and_backup(
    db: &Path,
    backup_dir: &Path,
    label: &str,
) -> anyhow::Result<Vec<PathBuf>> {
    {
        let p = create_pool(&db.to_string_lossy()).await?;
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .fetch_optional(&p)
            .await?;
        p.close().await;
    }
    std::fs::create_dir_all(backup_dir)?;
    let mut copies = Vec::new();
    let dst = backup_dir.join(format!("{label}-altevra.db"));
    std::fs::copy(db, &dst)?;
    copies.push(dst);
    for ext in ["-wal", "-shm"] {
        let side = sibling(db, ext);
        if side.exists() {
            let dst = backup_dir.join(format!("{label}-altevra.db{ext}"));
            std::fs::copy(&side, &dst)?;
            copies.push(dst);
        }
    }
    Ok(copies)
}

/// 033→034 upgrade for an attached shadow: `PRAGMA table_info` introspection +
/// targeted `ALTER TABLE … ADD COLUMN working_dir TEXT`, mirroring migration
/// 034 (including the turn backfill). Idempotent. NEVER the sqlx migrator.
async fn upgrade_shadow_schema(conn: &mut SqliteConnection, alias: &str) -> anyhow::Result<()> {
    for table in ["sessions", "turns"] {
        if !table_exists(conn, alias, table).await? {
            anyhow::bail!("{alias}: not an Altevra DB — table `{table}` missing");
        }
        let cols = table_cols(conn, alias, table).await?;
        if !cols.iter().any(|c| c == "working_dir") {
            sqlx::query(&format!(
                "ALTER TABLE {alias}.{table} ADD COLUMN working_dir TEXT"
            ))
            .execute(&mut *conn)
            .await?;
        }
    }
    // Mirror 034's backfill: turns inherit their session's working_dir.
    sqlx::query(&format!(
        "UPDATE {alias}.turns SET working_dir = (SELECT s.working_dir FROM {alias}.sessions s \
         WHERE s.id = {alias}.turns.session_id) WHERE working_dir IS NULL"
    ))
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn table_exists(
    conn: &mut SqliteConnection,
    alias: &str,
    table: &str,
) -> anyhow::Result<bool> {
    let row: Option<i64> = sqlx::query_scalar(&format!(
        "SELECT 1 FROM {alias}.sqlite_master WHERE type IN ('table','view') AND name = ?"
    ))
    .bind(table)
    .fetch_optional(&mut *conn)
    .await?;
    // FTS5 virtual tables register as type='table' too; this covers them.
    Ok(row.is_some())
}

async fn table_cols(
    conn: &mut SqliteConnection,
    alias: &str,
    table: &str,
) -> anyhow::Result<Vec<String>> {
    let rows = sqlx::query(&format!("PRAGMA {alias}.table_info({table})"))
        .fetch_all(&mut *conn)
        .await?;
    Ok(rows.into_iter().map(|r| r.get::<String, _>("name")).collect())
}

// ---------------------------------------------------------------------------
// In-memory row shapes + fingerprints
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct USession {
    id: String,
    tool: String,
    project_id: Option<String>,
    project_name: Option<String>,
    started_at: String,
    ended_at: Option<String>,
    summary: Option<String>,
    tokens_in_total: i64,
    tokens_out_total: i64,
    cost_usd_estimate: f64,
    metadata: String,
    external_id: Option<String>,
    imported_from: Option<String>,
    working_dir: Option<String>,
}

#[derive(Debug, Clone)]
struct UTurn {
    id: String,
    session_id: String,
    turn_idx: i64,
    role: String,
    content: String,
    tool_calls: Option<String>,
    tool_name: Option<String>,
    model: Option<String>,
    tokens_in: Option<i64>,
    tokens_out: Option<i64>,
    latency_ms: Option<i64>,
    file_changes: Option<String>,
    redacted_count: i64,
    source_tool: Option<String>,
    sensitivity: String,
    redaction_status: String,
    created_at: String,
    working_dir: Option<String>,
}

fn sha_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    format!("{:x}", h.finalize())
}

/// Full turn fingerprint over (role, content, tool_calls, file_changes) —
/// computed on the AS-STORED values, used for both collapse decisions and
/// the ordered turn-sequence hash.
fn turn_fp(role: &str, content: &str, tool_calls: Option<&str>, file_changes: Option<&str>) -> String {
    sha_hex(&format!(
        "{role}\u{1}{content}\u{1}{}\u{1}{}",
        tool_calls.unwrap_or(""),
        file_changes.unwrap_or("")
    ))
}

#[derive(Debug, Clone)]
struct FtsRow {
    object_type: String,
    object_id: String,
    title: String,
    body: String,
    tags: String,
}

/// Canonical-side dedup state, kept in memory and updated as the (single)
/// merge transaction inserts rows — so a second shadow sees the first
/// shadow's merges.
struct CanonState {
    /// session id → tool
    sessions_by_id: HashMap<String, String>,
    /// (tool, external_id) → session id
    sessions_by_ext: HashMap<(String, String), String>,
    /// session id → (tool, started_at)
    session_meta: HashMap<String, (String, String)>,
    /// session id → ordered per-turn fingerprints
    turn_seqs: HashMap<String, Vec<String>>,
    /// (session id, turn_idx) → (turn id, fingerprint)
    turns_by_key: HashMap<(String, i64), (String, String)>,
    turn_ids: HashSet<String>,
}

async fn load_canon_state(conn: &mut SqliteConnection) -> anyhow::Result<CanonState> {
    let mut state = CanonState {
        sessions_by_id: HashMap::new(),
        sessions_by_ext: HashMap::new(),
        session_meta: HashMap::new(),
        turn_seqs: HashMap::new(),
        turns_by_key: HashMap::new(),
        turn_ids: HashSet::new(),
    };
    let rows = sqlx::query("SELECT id, tool, external_id, started_at FROM main.sessions")
        .fetch_all(&mut *conn)
        .await?;
    for r in rows {
        let id: String = r.get("id");
        let tool: String = r.get("tool");
        let ext: Option<String> = r.get("external_id");
        let started: String = r.get("started_at");
        if let Some(e) = ext {
            state.sessions_by_ext.insert((tool.clone(), e), id.clone());
        }
        state.session_meta.insert(id.clone(), (tool.clone(), started));
        state.sessions_by_id.insert(id, tool);
    }
    let rows = sqlx::query(
        "SELECT id, session_id, turn_idx, role, content, tool_calls, file_changes \
         FROM main.turns ORDER BY session_id, turn_idx",
    )
    .fetch_all(&mut *conn)
    .await?;
    for r in rows {
        let id: String = r.get("id");
        let sid: String = r.get("session_id");
        let idx: i64 = r.get("turn_idx");
        let fp = turn_fp(
            &r.get::<String, _>("role"),
            &r.get::<String, _>("content"),
            r.get::<Option<String>, _>("tool_calls").as_deref(),
            r.get::<Option<String>, _>("file_changes").as_deref(),
        );
        state.turn_seqs.entry(sid.clone()).or_default().push(fp.clone());
        state.turns_by_key.insert((sid, idx), (id.clone(), fp));
        state.turn_ids.insert(id);
    }
    Ok(state)
}

// ---------------------------------------------------------------------------
// The merge body (runs inside the single canonical-only transaction)
// ---------------------------------------------------------------------------

async fn merge_all(
    conn: &mut SqliteConnection,
    n_shadows: usize,
    shadow_labels: &[PathBuf],
    report: &mut UnifyReport,
) -> anyhow::Result<Vec<FtsRow>> {
    report.counts_before = count_tables(conn).await?;
    let mut state = load_canon_state(conn).await?;
    let mut fts_rows: Vec<FtsRow> = Vec::new();

    for i in 0..n_shadows {
        let alias = format!("sh{i}");
        let label = shadow_labels
            .get(i)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| alias.clone());
        merge_one_shadow(conn, &alias, &label, &mut state, report, &mut fts_rows).await?;
    }

    // FK integrity gate: the merge must leave canonical referentially clean.
    let violations = sqlx::query("PRAGMA main.foreign_key_check")
        .fetch_all(&mut *conn)
        .await?;
    if !violations.is_empty() {
        anyhow::bail!(
            "foreign_key_check reported {} violation(s) after merge — rolling back",
            violations.len()
        );
    }

    report.counts_after = count_tables(conn).await?;
    Ok(fts_rows)
}

async fn count_tables(conn: &mut SqliteConnection) -> anyhow::Result<BTreeMap<String, i64>> {
    let mut out = BTreeMap::new();
    for t in COUNT_TABLES {
        let n: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM main.{t}"))
            .fetch_one(&mut *conn)
            .await?;
        out.insert((*t).to_string(), n);
    }
    Ok(out)
}

enum SessionPlan {
    MergeInto(String),
    New,
    Quarantine(String),
}

async fn merge_one_shadow(
    conn: &mut SqliteConnection,
    alias: &str,
    label: &str,
    state: &mut CanonState,
    report: &mut UnifyReport,
    fts_rows: &mut Vec<FtsRow>,
) -> anyhow::Result<()> {
    // ---- read shadow sessions + turns ----
    let sessions = read_sessions(conn, alias).await?;
    let mut turns_by_session: HashMap<String, Vec<UTurn>> = HashMap::new();
    for t in read_turns(conn, alias).await? {
        turns_by_session.entry(t.session_id.clone()).or_default().push(t);
    }
    for v in turns_by_session.values_mut() {
        v.sort_by_key(|t| t.turn_idx);
    }

    // session_map: shadow session id → canonical session id (merge targets +
    // identity entries for new sessions). Quarantined sessions are ABSENT.
    let mut session_map: HashMap<String, String> = HashMap::new();
    let mut quarantined_sessions: HashSet<String> = HashSet::new();
    // turn_map: shadow turn id → canonical turn id (collapsed duplicates).
    let mut turn_map: HashMap<String, String> = HashMap::new();

    for sess in &sessions {
        let empty: Vec<UTurn> = Vec::new();
        let s_turns = turns_by_session.get(&sess.id).unwrap_or(&empty);
        let s_seq: Vec<String> = s_turns
            .iter()
            .map(|t| turn_fp(&t.role, &t.content, t.tool_calls.as_deref(), t.file_changes.as_deref()))
            .collect();

        let plan = plan_session(sess, &s_seq, state);
        let target = match plan {
            SessionPlan::Quarantine(reason) => {
                report.sessions_quarantined += 1;
                report.conflicts.push(format!(
                    "[{label}] session {} ({}, started {}) QUARANTINED (left in shadow): {reason}",
                    sess.id, sess.tool, sess.started_at
                ));
                quarantined_sessions.insert(sess.id.clone());
                continue;
            }
            SessionPlan::MergeInto(target) => {
                report.sessions_merged += 1;
                // Backfill ended_at/summary on the kept session when missing.
                sqlx::query(
                    "UPDATE main.sessions SET ended_at = COALESCE(ended_at, ?), \
                     summary = COALESCE(summary, ?), \
                     working_dir = COALESCE(working_dir, ?) WHERE id = ?",
                )
                .bind(sess.ended_at.as_deref())
                .bind(sess.summary.as_deref())
                .bind(sess.working_dir.as_deref())
                .bind(&target)
                .execute(&mut *conn)
                .await?;
                target
            }
            SessionPlan::New => {
                report.sessions_new += 1;
                insert_session(conn, sess).await?;
                state.sessions_by_id.insert(sess.id.clone(), sess.tool.clone());
                state
                    .session_meta
                    .insert(sess.id.clone(), (sess.tool.clone(), sess.started_at.clone()));
                if let Some(e) = &sess.external_id {
                    state
                        .sessions_by_ext
                        .insert((sess.tool.clone(), e.clone()), sess.id.clone());
                }
                sess.id.clone()
            }
        };
        session_map.insert(sess.id.clone(), target.clone());

        // ---- turn merge under the target session ----
        let mut tokens_in_added: i64 = 0;
        let mut tokens_out_added: i64 = 0;
        let mut touched = false;
        for (t, fp) in s_turns.iter().zip(s_seq.iter()) {
            let key = (target.clone(), t.turn_idx);
            if let Some((canon_id, canon_fp)) = state.turns_by_key.get(&key) {
                if canon_fp == fp {
                    report.turns_collapsed += 1;
                    turn_map.insert(t.id.clone(), canon_id.clone());
                } else {
                    report.turns_quarantined += 1;
                    report.conflicts.push(format!(
                        "[{label}] turn (session {}, idx {}) DIVERGES from canonical → turns_quarantine",
                        target, t.turn_idx
                    ));
                    insert_quarantined_turn(conn, t, &target, label, "divergent_turn_idx").await?;
                }
                continue;
            }
            if state.turn_ids.contains(&t.id) {
                report.turns_quarantined += 1;
                report.conflicts.push(format!(
                    "[{label}] turn id {} collides with a different canonical turn → turns_quarantine",
                    t.id
                ));
                insert_quarantined_turn(conn, t, &target, label, "turn_id_collision").await?;
                continue;
            }
            insert_turn(conn, t, &target).await?;
            report.turns_inserted += 1;
            touched = true;
            tokens_in_added += t.tokens_in.unwrap_or(0);
            tokens_out_added += t.tokens_out.unwrap_or(0);
            state
                .turn_seqs
                .entry(target.clone())
                .or_default()
                .push(fp.clone());
            state.turns_by_key.insert(key, (t.id.clone(), fp.clone()));
            state.turn_ids.insert(t.id.clone());
        }

        // Recount aggregates on the kept session. New sessions were inserted
        // with the shadow's token totals, so only add tokens for merges.
        if touched {
            let is_new = target == sess.id;
            if is_new {
                sqlx::query(
                    "UPDATE main.sessions SET turn_count = \
                     (SELECT COUNT(*) FROM main.turns WHERE session_id = ?) WHERE id = ?",
                )
                .bind(&target)
                .bind(&target)
                .execute(&mut *conn)
                .await?;
            } else {
                sqlx::query(
                    "UPDATE main.sessions SET \
                     turn_count = (SELECT COUNT(*) FROM main.turns WHERE session_id = ?), \
                     tokens_in_total = tokens_in_total + ?, \
                     tokens_out_total = tokens_out_total + ? WHERE id = ?",
                )
                .bind(&target)
                .bind(tokens_in_added)
                .bind(tokens_out_added)
                .bind(&target)
                .execute(&mut *conn)
                .await?;
            }
        }
    }

    // ---- file_changes (FK remap: session_id + turn_id) ----
    merge_file_changes(conn, alias, label, &session_map, &turn_map, &quarantined_sessions, report)
        .await?;

    // ---- events (FK remap: entity_id) ----
    merge_events(conn, alias, &session_map, &turn_map, report).await?;

    // ---- improvement_signals (payload refs in source_ref) ----
    merge_signals(conn, alias, &session_map, &turn_map, report).await?;

    // ---- proposals (evidence_refs JSON) ----
    merge_proposals(conn, alias, label, &session_map, &turn_map, report).await?;

    // ---- FTS reindex list: objects about to be NEWLY inserted ----
    if table_exists(conn, alias, "object_index").await?
        && table_exists(conn, alias, "object_fts").await?
    {
        let rows = sqlx::query(&format!(
            "SELECT s.type AS otype, s.id AS oid, \
                    COALESCE(f.title, COALESCE(s.title, '')) AS title, \
                    COALESCE(f.body, '') AS body, \
                    COALESCE(f.tags, s.tags) AS tags \
             FROM {alias}.object_index s \
             LEFT JOIN {alias}.object_fts f \
               ON f.object_type = s.type AND f.object_id = s.id \
             WHERE NOT EXISTS (SELECT 1 FROM main.object_index m \
                               WHERE m.type = s.type AND m.id = s.id)"
        ))
        .fetch_all(&mut *conn)
        .await?;
        for r in rows {
            fts_rows.push(FtsRow {
                object_type: r.get("otype"),
                object_id: r.get("oid"),
                title: r.get("title"),
                body: r.get("body"),
                tags: r.get("tags"),
            });
        }
    }

    // ---- generic content tables (dedup by primary id, quarantine-on-diff) ----
    for (table, pk, guard) in CONTENT_TABLES {
        let counts = merge_content_table(conn, alias, table, pk, *guard).await?;
        if counts.id_conflicts > 0 {
            report.conflicts.push(format!(
                "[{label}] {table}: {} id-collision(s) with DIFFERENT content — left in shadow",
                counts.id_conflicts
            ));
        }
        if counts.unique_collisions > 0 {
            report.conflicts.push(format!(
                "[{label}] {table}: {} unique-key collision(s) under a different id — left in shadow",
                counts.unique_collisions
            ));
        }
        let entry = report.content.entry((*table).to_string()).or_default();
        entry.inserted += counts.inserted;
        entry.identical += counts.identical;
        entry.id_conflicts += counts.id_conflicts;
        entry.unique_collisions += counts.unique_collisions;
    }

    Ok(())
}

/// Locked conservative dedup decision for one shadow session.
fn plan_session(sess: &USession, s_seq: &[String], state: &CanonState) -> SessionPlan {
    // 1. session-id match → merge (strongest signal).
    if state.sessions_by_id.contains_key(&sess.id) {
        return SessionPlan::MergeInto(sess.id.clone());
    }
    // 2. (tool, external_id) → merge.
    if let Some(ext) = &sess.external_id {
        if let Some(target) = state
            .sessions_by_ext
            .get(&(sess.tool.clone(), ext.clone()))
        {
            return SessionPlan::MergeInto(target.clone());
        }
        // Explicit external identity, no counterpart → genuinely new.
        return SessionPlan::New;
    }
    // 3. NULL external_id: full ordered NON-EMPTY turn-sequence hash.
    if !s_seq.is_empty() {
        for (sid, (tool, _)) in &state.session_meta {
            if tool != &sess.tool {
                continue;
            }
            if let Some(c_seq) = state.turn_seqs.get(sid) {
                if c_seq == s_seq {
                    return SessionPlan::MergeInto(sid.clone());
                }
            }
        }
    }
    // 4. Partial / ambiguous similarity → quarantine, never auto-merge.
    for (sid, (tool, started)) in &state.session_meta {
        if tool != &sess.tool {
            continue;
        }
        let c_seq = state.turn_seqs.get(sid).cloned().unwrap_or_default();
        let prefix_related = !s_seq.is_empty()
            && !c_seq.is_empty()
            && s_seq != &c_seq[..]
            && (c_seq.starts_with(s_seq) || s_seq.starts_with(&c_seq[..]));
        let same_start = started == &sess.started_at;
        if prefix_related {
            return SessionPlan::Quarantine(format!(
                "turn sequence is a partial match of canonical session {sid} \
                 (prefix overlap, not equal)"
            ));
        }
        if same_start {
            return SessionPlan::Quarantine(format!(
                "same tool + identical started_at as canonical session {sid} but \
                 different turn sequence"
            ));
        }
    }
    SessionPlan::New
}

async fn read_sessions(conn: &mut SqliteConnection, alias: &str) -> anyhow::Result<Vec<USession>> {
    let rows = sqlx::query(&format!(
        "SELECT id, tool, project_id, project_name, started_at, ended_at, summary, \
                tokens_in_total, tokens_out_total, cost_usd_estimate, \
                metadata, external_id, imported_from, working_dir \
         FROM {alias}.sessions ORDER BY started_at, id"
    ))
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| USession {
            id: r.get("id"),
            tool: r.get("tool"),
            project_id: r.get("project_id"),
            project_name: r.get("project_name"),
            started_at: r.get("started_at"),
            ended_at: r.get("ended_at"),
            summary: r.get("summary"),
            tokens_in_total: r.get("tokens_in_total"),
            tokens_out_total: r.get("tokens_out_total"),
            cost_usd_estimate: r.get("cost_usd_estimate"),
            metadata: r.get("metadata"),
            external_id: r.get("external_id"),
            imported_from: r.get("imported_from"),
            working_dir: r.get("working_dir"),
        })
        .collect())
}

async fn read_turns(conn: &mut SqliteConnection, alias: &str) -> anyhow::Result<Vec<UTurn>> {
    let rows = sqlx::query(&format!(
        "SELECT id, session_id, turn_idx, role, content, tool_calls, tool_name, model, \
                tokens_in, tokens_out, latency_ms, file_changes, redacted_count, \
                source_tool, sensitivity, redaction_status, created_at, working_dir \
         FROM {alias}.turns ORDER BY session_id, turn_idx"
    ))
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| UTurn {
            id: r.get("id"),
            session_id: r.get("session_id"),
            turn_idx: r.get("turn_idx"),
            role: r.get("role"),
            content: r.get("content"),
            tool_calls: r.get("tool_calls"),
            tool_name: r.get("tool_name"),
            model: r.get("model"),
            tokens_in: r.get("tokens_in"),
            tokens_out: r.get("tokens_out"),
            latency_ms: r.get("latency_ms"),
            file_changes: r.get("file_changes"),
            redacted_count: r.get("redacted_count"),
            source_tool: r.get("source_tool"),
            sensitivity: r.get("sensitivity"),
            redaction_status: r.get("redaction_status"),
            created_at: r.get("created_at"),
            working_dir: r.get("working_dir"),
        })
        .collect())
}

async fn insert_session(conn: &mut SqliteConnection, s: &USession) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO main.sessions \
         (id, tool, project_id, project_name, started_at, ended_at, summary, \
          tokens_in_total, tokens_out_total, cost_usd_estimate, turn_count, metadata, \
          external_id, imported_from, working_dir) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&s.id)
    .bind(&s.tool)
    .bind(s.project_id.as_deref())
    .bind(s.project_name.as_deref())
    .bind(&s.started_at)
    .bind(s.ended_at.as_deref())
    .bind(s.summary.as_deref())
    .bind(s.tokens_in_total)
    .bind(s.tokens_out_total)
    .bind(s.cost_usd_estimate)
    .bind(0_i64) // recounted after turn inserts
    .bind(&s.metadata)
    .bind(s.external_id.as_deref())
    .bind(s.imported_from.as_deref())
    .bind(s.working_dir.as_deref())
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Re-guard verdicts for a shadow turn being inserted into canonical. 033-era
/// rows predate current redaction hardening, so content + side channels go
/// back through guard_text/guard_json; a row that cannot keep a clean/redacted
/// verdict is marked `unscanned` (ExposureGate fail-closes on it).
struct Reguarded {
    content: String,
    tool_calls: Option<String>,
    file_changes: Option<String>,
    redacted_count: i64,
    sensitivity: String,
    redaction_status: String,
}

fn reguard_turn(t: &UTurn) -> Reguarded {
    use altevra_core::status::RedactionStatus;
    use altevra_core::Sensitivity;

    let declared: Sensitivity = t.sensitivity.parse().unwrap_or(Sensitivity::Internal);
    let g = guard_text(&t.content, declared);
    let mut redacted_count = t.redacted_count.max(g.sightings.len() as i64);
    let mut sensitivity = g.sensitivity.clone();
    let mut any_redacted = matches!(g.redaction_status, RedactionStatus::Redacted);

    let mut guard_side = |raw: Option<&str>| -> Option<String> {
        let raw = raw?;
        match serde_json::from_str::<serde_json::Value>(raw) {
            Ok(v) => {
                let (scrubbed, n, sens) = guard_json(&v);
                redacted_count += n;
                sensitivity = sensitivity.combine(&sens);
                any_redacted |= n > 0;
                Some(scrubbed.to_string())
            }
            Err(_) => {
                let gt = guard_text(raw, Sensitivity::Internal);
                any_redacted |= matches!(gt.redaction_status, RedactionStatus::Redacted);
                sensitivity = sensitivity.combine(&gt.sensitivity);
                Some(gt.value)
            }
        }
    };
    let tool_calls = guard_side(t.tool_calls.as_deref());
    let file_changes = guard_side(t.file_changes.as_deref());

    let redaction_status = if any_redacted {
        RedactionStatus::Redacted.to_string()
    } else if t.redaction_status == "clean" || t.redaction_status == "redacted" {
        t.redaction_status.clone()
    } else {
        RedactionStatus::Unscanned.to_string()
    };

    Reguarded {
        content: g.value,
        tool_calls,
        file_changes,
        redacted_count,
        sensitivity: sensitivity.to_string(),
        redaction_status,
    }
}

async fn insert_turn(
    conn: &mut SqliteConnection,
    t: &UTurn,
    target_session: &str,
) -> anyhow::Result<()> {
    let rg = reguard_turn(t);
    sqlx::query(
        "INSERT INTO main.turns \
         (id, session_id, turn_idx, role, content, tool_calls, tool_name, model, \
          tokens_in, tokens_out, latency_ms, file_changes, redacted_count, \
          source_tool, sensitivity, redaction_status, created_at, working_dir) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&t.id)
    .bind(target_session)
    .bind(t.turn_idx)
    .bind(&t.role)
    .bind(&rg.content)
    .bind(rg.tool_calls.as_deref())
    .bind(t.tool_name.as_deref())
    .bind(t.model.as_deref())
    .bind(t.tokens_in)
    .bind(t.tokens_out)
    .bind(t.latency_ms)
    .bind(rg.file_changes.as_deref())
    .bind(rg.redacted_count)
    .bind(t.source_tool.as_deref())
    .bind(&rg.sensitivity)
    .bind(&rg.redaction_status)
    .bind(&t.created_at)
    .bind(t.working_dir.as_deref())
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn insert_quarantined_turn(
    conn: &mut SqliteConnection,
    t: &UTurn,
    target_session: &str,
    source_db: &str,
    reason: &str,
) -> anyhow::Result<()> {
    let rg = reguard_turn(t);
    sqlx::query(
        "INSERT INTO main.turns_quarantine \
         (id, original_turn_id, session_id, turn_idx, role, content, tool_calls, \
          tool_name, model, tokens_in, tokens_out, latency_ms, file_changes, \
          redacted_count, source_tool, sensitivity, redaction_status, created_at, \
          working_dir, source_db, reason) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(&t.id)
    .bind(target_session)
    .bind(t.turn_idx)
    .bind(&t.role)
    .bind(&rg.content)
    .bind(rg.tool_calls.as_deref())
    .bind(t.tool_name.as_deref())
    .bind(t.model.as_deref())
    .bind(t.tokens_in)
    .bind(t.tokens_out)
    .bind(t.latency_ms)
    .bind(rg.file_changes.as_deref())
    .bind(rg.redacted_count)
    .bind(t.source_tool.as_deref())
    .bind(&rg.sensitivity)
    .bind(&rg.redaction_status)
    .bind(&t.created_at)
    .bind(t.working_dir.as_deref())
    .bind(source_db)
    .bind(reason)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn merge_file_changes(
    conn: &mut SqliteConnection,
    alias: &str,
    label: &str,
    session_map: &HashMap<String, String>,
    turn_map: &HashMap<String, String>,
    quarantined_sessions: &HashSet<String>,
    report: &mut UnifyReport,
) -> anyhow::Result<()> {
    if !table_exists(conn, alias, "file_changes").await? {
        return Ok(());
    }
    let rows = sqlx::query(&format!(
        "SELECT id, session_id, turn_id, path, before_hash, after_hash, diff_summary, \
                actor_type, actor_id, created_at FROM {alias}.file_changes"
    ))
    .fetch_all(&mut *conn)
    .await?;
    for r in rows {
        let id: String = r.get("id");
        let sid: Option<String> = r.get("session_id");
        let tid: Option<String> = r.get("turn_id");

        if let Some(s) = &sid {
            if quarantined_sessions.contains(s) {
                report.file_changes_skipped += 1;
                continue; // its session stayed in the shadow — so does this row
            }
        }
        let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM main.file_changes WHERE id = ?")
            .bind(&id)
            .fetch_optional(&mut *conn)
            .await?;
        if exists.is_some() {
            report.file_changes_skipped += 1;
            continue;
        }
        // FK remap: session_id + turn_id follow their merged owners.
        let sid = sid.map(|s| session_map.get(&s).cloned().unwrap_or(s));
        let tid = tid.map(|t| turn_map.get(&t).cloned().unwrap_or(t));
        // A turn that went to quarantine has no canonical row — NULL the ref
        // rather than violating the FK (the quarantine row keeps the evidence).
        let tid = match tid {
            Some(t) => {
                let ok: Option<i64> = sqlx::query_scalar("SELECT 1 FROM main.turns WHERE id = ?")
                    .bind(&t)
                    .fetch_optional(&mut *conn)
                    .await?;
                if ok.is_some() {
                    Some(t)
                } else {
                    report.conflicts.push(format!(
                        "[{label}] file_change {id}: turn ref {t} not in canonical (quarantined?) → turn_id set NULL"
                    ));
                    None
                }
            }
            None => None,
        };
        sqlx::query(
            "INSERT INTO main.file_changes \
             (id, session_id, turn_id, path, before_hash, after_hash, diff_summary, \
              actor_type, actor_id, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(sid.as_deref())
        .bind(tid.as_deref())
        .bind(r.get::<String, _>("path"))
        .bind(r.get::<Option<String>, _>("before_hash"))
        .bind(r.get::<Option<String>, _>("after_hash"))
        .bind(r.get::<Option<String>, _>("diff_summary"))
        .bind(r.get::<String, _>("actor_type"))
        .bind(r.get::<Option<String>, _>("actor_id"))
        .bind(r.get::<String, _>("created_at"))
        .execute(&mut *conn)
        .await?;
        report.file_changes_inserted += 1;
    }
    Ok(())
}

async fn merge_events(
    conn: &mut SqliteConnection,
    alias: &str,
    session_map: &HashMap<String, String>,
    turn_map: &HashMap<String, String>,
    report: &mut UnifyReport,
) -> anyhow::Result<()> {
    if !table_exists(conn, alias, "events").await? {
        return Ok(());
    }
    let rows = sqlx::query(&format!(
        "SELECT id, event_type, project_id, actor_type, actor_id, source, entity_type, \
                entity_id, title, summary, payload, sensitivity, created_at, \
                processed_at, status FROM {alias}.events"
    ))
    .fetch_all(&mut *conn)
    .await?;
    for r in rows {
        let id: String = r.get("id");
        let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM main.events WHERE id = ?")
            .bind(&id)
            .fetch_optional(&mut *conn)
            .await?;
        if exists.is_some() {
            report.events_skipped += 1;
            continue;
        }
        // FK remap: entity_id pointing at a merged session/turn follows it.
        let entity_id: Option<String> = r.get("entity_id");
        let entity_id = entity_id.map(|e| {
            session_map
                .get(&e)
                .or_else(|| turn_map.get(&e))
                .cloned()
                .unwrap_or(e)
        });
        sqlx::query(
            "INSERT INTO main.events \
             (id, event_type, project_id, actor_type, actor_id, source, entity_type, \
              entity_id, title, summary, payload, sensitivity, created_at, processed_at, \
              status) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(r.get::<String, _>("event_type"))
        .bind(r.get::<Option<String>, _>("project_id"))
        .bind(r.get::<String, _>("actor_type"))
        .bind(r.get::<Option<String>, _>("actor_id"))
        .bind(r.get::<String, _>("source"))
        .bind(r.get::<Option<String>, _>("entity_type"))
        .bind(entity_id.as_deref())
        .bind(r.get::<String, _>("title"))
        .bind(r.get::<Option<String>, _>("summary"))
        .bind(r.get::<String, _>("payload"))
        .bind(r.get::<String, _>("sensitivity"))
        .bind(r.get::<String, _>("created_at"))
        .bind(r.get::<Option<String>, _>("processed_at"))
        .bind(r.get::<String, _>("status"))
        .execute(&mut *conn)
        .await?;
        report.events_inserted += 1;
    }
    Ok(())
}

/// Replace every occurrence of a remapped id inside a ref string (refs use
/// `session:<uuid>` / `turn:<uuid>` shapes; UUIDs are substring-safe).
fn remap_refs(
    raw: &str,
    session_map: &HashMap<String, String>,
    turn_map: &HashMap<String, String>,
) -> String {
    let mut out = raw.to_string();
    for (old, new) in session_map.iter().chain(turn_map.iter()) {
        if old != new && out.contains(old.as_str()) {
            out = out.replace(old.as_str(), new.as_str());
        }
    }
    out
}

async fn merge_signals(
    conn: &mut SqliteConnection,
    alias: &str,
    session_map: &HashMap<String, String>,
    turn_map: &HashMap<String, String>,
    report: &mut UnifyReport,
) -> anyhow::Result<()> {
    if !table_exists(conn, alias, "improvement_signals").await? {
        return Ok(());
    }
    let rows = sqlx::query(&format!(
        "SELECT id, kind, source_ref, summary, cluster_key, created_at \
         FROM {alias}.improvement_signals"
    ))
    .fetch_all(&mut *conn)
    .await?;
    for r in rows {
        let id: String = r.get("id");
        let kind: String = r.get("kind");
        let source_ref = remap_refs(&r.get::<String, _>("source_ref"), session_map, turn_map);
        let summary: String = r.get("summary");
        let exists: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM main.improvement_signals \
             WHERE id = ? OR (kind = ? AND source_ref = ? AND summary = ?)",
        )
        .bind(&id)
        .bind(&kind)
        .bind(&source_ref)
        .bind(&summary)
        .fetch_optional(&mut *conn)
        .await?;
        if exists.is_some() {
            report.signals_skipped += 1;
            continue;
        }
        sqlx::query(
            "INSERT INTO main.improvement_signals \
             (id, kind, source_ref, summary, cluster_key, created_at) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&kind)
        .bind(&source_ref)
        .bind(&summary)
        .bind(r.get::<Option<String>, _>("cluster_key"))
        .bind(r.get::<String, _>("created_at"))
        .execute(&mut *conn)
        .await?;
        report.signals_inserted += 1;
    }
    Ok(())
}

async fn merge_proposals(
    conn: &mut SqliteConnection,
    alias: &str,
    label: &str,
    session_map: &HashMap<String, String>,
    turn_map: &HashMap<String, String>,
    report: &mut UnifyReport,
) -> anyhow::Result<()> {
    if !table_exists(conn, alias, "proposals").await? {
        return Ok(());
    }
    let rows = sqlx::query(&format!(
        "SELECT id, kind, risk_tier, status, title, body, source_mode, dedup_hash, \
                evidence_count, evidence_refs, decided_by, decided_at, created_at \
         FROM {alias}.proposals"
    ))
    .fetch_all(&mut *conn)
    .await?;
    for r in rows {
        let id: String = r.get("id");
        let dedup_hash: String = r.get("dedup_hash");
        let title: String = r.get("title");

        let by_id: Option<String> =
            sqlx::query_scalar("SELECT dedup_hash FROM main.proposals WHERE id = ?")
                .bind(&id)
                .fetch_optional(&mut *conn)
                .await?;
        if let Some(existing_hash) = by_id {
            report.proposals_skipped += 1;
            if existing_hash != dedup_hash {
                report.conflicts.push(format!(
                    "[{label}] proposal {id} collides with a canonical proposal of \
                     DIFFERENT content — left in shadow"
                ));
            }
            continue;
        }
        let by_hash: Option<i64> =
            sqlx::query_scalar("SELECT 1 FROM main.proposals WHERE dedup_hash = ?")
                .bind(&dedup_hash)
                .fetch_optional(&mut *conn)
                .await?;
        if by_hash.is_some() {
            report.proposals_skipped += 1;
            report.conflicts.push(format!(
                "[{label}] proposal {id} ('{title}') dedup_hash already present under a \
                 different id — left in shadow"
            ));
            continue;
        }
        // FK remap inside the evidence_refs JSON.
        let evidence_refs =
            remap_refs(&r.get::<String, _>("evidence_refs"), session_map, turn_map);
        sqlx::query(
            "INSERT INTO main.proposals \
             (id, kind, risk_tier, status, title, body, source_mode, dedup_hash, \
              evidence_count, evidence_refs, decided_by, decided_at, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(r.get::<String, _>("kind"))
        .bind(r.get::<String, _>("risk_tier"))
        .bind(r.get::<String, _>("status"))
        .bind(&title)
        .bind(r.get::<String, _>("body"))
        .bind(r.get::<Option<String>, _>("source_mode"))
        .bind(&dedup_hash)
        .bind(r.get::<i64, _>("evidence_count"))
        .bind(&evidence_refs)
        .bind(r.get::<Option<String>, _>("decided_by"))
        .bind(r.get::<Option<String>, _>("decided_at"))
        .bind(r.get::<String, _>("created_at"))
        .execute(&mut *conn)
        .await?;
        report.proposals_inserted += 1;
    }
    Ok(())
}

/// Generic content-table merge: insert shadow rows whose primary id does not
/// exist in canonical (and whose secondary unique key, when given, does not
/// collide). Identical rows are skipped; id-collisions with DIFFERENT content
/// are counted as conflicts and left in the (quarantined) shadow.
async fn merge_content_table(
    conn: &mut SqliteConnection,
    alias: &str,
    table: &str,
    pk: &[&str],
    unique_guard: Option<&str>,
) -> anyhow::Result<ContentMergeCounts> {
    let mut counts = ContentMergeCounts::default();
    if !table_exists(conn, alias, table).await? {
        return Ok(counts);
    }
    let main_cols = table_cols(conn, "main", table).await?;
    let shadow_cols: HashSet<String> = table_cols(conn, alias, table).await?.into_iter().collect();
    let cols: Vec<String> = main_cols
        .into_iter()
        .filter(|c| shadow_cols.contains(c))
        .collect();
    if cols.is_empty() {
        return Ok(counts);
    }

    let pk_join = pk
        .iter()
        .map(|c| format!("m.{c} = s.{c}"))
        .collect::<Vec<_>>()
        .join(" AND ");
    let diff_pred = {
        let parts: Vec<String> = cols
            .iter()
            .filter(|c| !pk.contains(&c.as_str()))
            .map(|c| format!("m.{c} IS NOT s.{c}"))
            .collect();
        if parts.is_empty() {
            "0".to_string()
        } else {
            parts.join(" OR ")
        }
    };

    counts.id_conflicts = sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM {alias}.{table} s JOIN main.{table} m ON {pk_join} \
         WHERE ({diff_pred})"
    ))
    .fetch_one(&mut *conn)
    .await?;
    counts.identical = sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM {alias}.{table} s JOIN main.{table} m ON {pk_join} \
         WHERE NOT ({diff_pred})"
    ))
    .fetch_one(&mut *conn)
    .await?;

    let not_in_main = format!("NOT EXISTS (SELECT 1 FROM main.{table} m WHERE {pk_join})");
    if let Some(guard) = unique_guard {
        counts.unique_collisions = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM {alias}.{table} s WHERE {not_in_main} AND {guard}"
        ))
        .fetch_one(&mut *conn)
        .await?;
    }

    let col_list = cols.join(", ");
    let sel_list = cols
        .iter()
        .map(|c| format!("s.{c}"))
        .collect::<Vec<_>>()
        .join(", ");
    let guard_clause = unique_guard
        .map(|g| format!(" AND NOT {g}"))
        .unwrap_or_default();
    let res = sqlx::query(&format!(
        "INSERT INTO main.{table} ({col_list}) SELECT {sel_list} FROM {alias}.{table} s \
         WHERE {not_in_main}{guard_clause}"
    ))
    .execute(&mut *conn)
    .await?;
    counts.inserted = res.rows_affected();
    Ok(counts)
}

// ---------------------------------------------------------------------------
// Report printing
// ---------------------------------------------------------------------------

fn print_report(r: &UnifyReport) {
    println!();
    println!(
        "=== db unify {} ===",
        if r.applied { "APPLIED" } else { "DRY-RUN (rolled back — nothing changed)" }
    );
    println!("Shadows merged: {}", r.shadows.len());
    println!();
    println!("{:<22} {:>10} {:>10} {:>8}", "table", "before", "after", "delta");
    for (table, before) in &r.counts_before {
        let after = r.counts_after.get(table).copied().unwrap_or(*before);
        println!(
            "{:<22} {:>10} {:>10} {:>+8}",
            table,
            before,
            after,
            after - before
        );
    }
    println!();
    println!(
        "sessions: {} new, {} merged, {} quarantined (left in shadow)",
        r.sessions_new, r.sessions_merged, r.sessions_quarantined
    );
    println!(
        "turns:    {} inserted, {} collapsed (exact dup), {} → turns_quarantine",
        r.turns_inserted, r.turns_collapsed, r.turns_quarantined
    );
    println!(
        "file_changes: {} inserted, {} skipped; events: {} inserted, {} skipped",
        r.file_changes_inserted, r.file_changes_skipped, r.events_inserted, r.events_skipped
    );
    println!(
        "signals: {} inserted, {} skipped; proposals: {} inserted, {} skipped",
        r.signals_inserted, r.signals_skipped, r.proposals_inserted, r.proposals_skipped
    );
    for (table, c) in &r.content {
        println!(
            "{table}: {} inserted, {} identical, {} id-conflicts, {} unique-collisions",
            c.inserted, c.identical, c.id_conflicts, c.unique_collisions
        );
    }
    println!("FTS re-index: {} object(s)", r.fts_reindexed);

    if !r.conflicts.is_empty() {
        println!();
        println!("--- conflict report ({}) ---", r.conflicts.len());
        for c in &r.conflicts {
            println!("  {c}");
        }
    }
    if !r.backups.is_empty() {
        println!();
        println!("Backups:");
        for b in &r.backups {
            println!("  {}", b.display());
        }
    }
    if !r.quarantined_paths.is_empty() {
        println!();
        println!("Quarantined shadow DBs (renamed, NOT deleted):");
        for q in &r.quarantined_paths {
            println!("  {}", q.display());
        }
    }
}

// ===========================================================================
// Tests — hermetic, per-test temp DBs. NEVER the real ~/.altevra data.
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn fresh_db(path: &Path) -> SqlitePool {
        let pool = create_pool(&path.to_string_lossy()).await.unwrap();
        run_migrations(&pool).await.unwrap();
        pool
    }

    /// Build a 033-schema shadow: full migrations, then DROP the 034 columns
    /// so `working_dir` is missing from sessions AND turns — exactly the real
    /// shadow's shape. Unify must upgrade it via introspection.
    async fn make_shadow_033(path: &Path) -> SqlitePool {
        let pool = fresh_db(path).await;
        sqlx::query("ALTER TABLE sessions DROP COLUMN working_dir")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("ALTER TABLE turns DROP COLUMN working_dir")
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    async fn seed_session(
        pool: &SqlitePool,
        id: &str,
        tool: &str,
        external_id: Option<&str>,
        started_at: &str,
        has_working_dir_col: bool,
    ) {
        if has_working_dir_col {
            sqlx::query(
                "INSERT INTO sessions (id, tool, started_at, metadata, external_id, working_dir) \
                 VALUES (?, ?, ?, '{}', ?, NULL)",
            )
        } else {
            sqlx::query(
                "INSERT INTO sessions (id, tool, started_at, metadata, external_id) \
                 VALUES (?, ?, ?, '{}', ?)",
            )
        }
        .bind(id)
        .bind(tool)
        .bind(started_at)
        .bind(external_id)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn seed_turn(
        pool: &SqlitePool,
        id: &str,
        session_id: &str,
        idx: i64,
        content: &str,
        has_working_dir_col: bool,
    ) {
        if has_working_dir_col {
            sqlx::query(
                "INSERT INTO turns (id, session_id, turn_idx, role, content, redacted_count, \
                 sensitivity, redaction_status, created_at, working_dir) \
                 VALUES (?, ?, ?, 'user', ?, 0, 'internal', 'clean', \
                 '2026-06-01T10:00:00.000Z', NULL)",
            )
        } else {
            sqlx::query(
                "INSERT INTO turns (id, session_id, turn_idx, role, content, redacted_count, \
                 sensitivity, redaction_status, created_at) \
                 VALUES (?, ?, ?, 'user', ?, 0, 'internal', 'clean', \
                 '2026-06-01T10:00:00.000Z')",
            )
        }
        .bind(id)
        .bind(session_id)
        .bind(idx)
        .bind(content)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query("UPDATE sessions SET turn_count = turn_count + 1 WHERE id = ?")
            .bind(session_id)
            .execute(pool)
            .await
            .unwrap();
    }

    fn uid() -> String {
        Uuid::new_v4().to_string()
    }

    async fn count(pool: &SqlitePool, table: &str) -> i64 {
        sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(pool)
            .await
            .unwrap()
    }

    // -----------------------------------------------------------------------
    // unify: exact union counts + 033 introspection upgrade + FK integrity
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn unify_merges_033_shadow_exact_counts_and_upgrades_schema() {
        let tmp = TempDir::new().unwrap();
        let canonical = tmp.path().join("canonical.db");
        let shadow = tmp.path().join("proj/.altevra/altevra.db");
        std::fs::create_dir_all(shadow.parent().unwrap()).unwrap();

        // Canonical: 1 session, 2 turns.
        let cpool = fresh_db(&canonical).await;
        let cs = uid();
        seed_session(&cpool, &cs, "claude-code", None, "2026-06-01T09:00:00.000Z", true).await;
        seed_turn(&cpool, &uid(), &cs, 0, "canon turn 0", true).await;
        seed_turn(&cpool, &uid(), &cs, 1, "canon turn 1", true).await;
        cpool.close().await;

        // Shadow at 033 schema (no working_dir): 2 distinct sessions, 3 turns.
        let spool = make_shadow_033(&shadow).await;
        let s1 = uid();
        let s2 = uid();
        seed_session(&spool, &s1, "claude-code", Some("ext-1"), "2026-06-02T10:00:00.000Z", false)
            .await;
        seed_session(&spool, &s2, "codex", None, "2026-06-03T11:00:00.000Z", false).await;
        seed_turn(&spool, &uid(), &s1, 0, "shadow s1 turn 0", false).await;
        seed_turn(&spool, &uid(), &s1, 1, "shadow s1 turn 1", false).await;
        seed_turn(&spool, &uid(), &s2, 0, "shadow s2 turn 0", false).await;
        spool.close().await;

        let report = unify(&UnifyOptions {
            canonical: canonical.clone(),
            shadows: vec![shadow.clone()],
            apply: true,
            backup_root: tmp.path().join("backups"),
        })
        .await
        .unwrap();

        assert_eq!(report.sessions_new, 2);
        assert_eq!(report.sessions_merged, 0);
        assert_eq!(report.sessions_quarantined, 0);
        assert_eq!(report.turns_inserted, 3);
        assert_eq!(report.turns_collapsed, 0);
        assert_eq!(report.turns_quarantined, 0);

        let pool = fresh_db(&canonical).await;
        assert_eq!(count(&pool, "sessions").await, 3, "exact union: 1 + 2");
        assert_eq!(count(&pool, "turns").await, 5, "exact union: 2 + 3");

        // FK integrity after merge.
        let violations = sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert!(violations.is_empty(), "no FK violations after unify");

        // Merged sessions/turns are readable INCLUDING working_dir (033→034
        // introspection upgrade happened on the shadow before the merge).
        let wd: Option<String> =
            sqlx::query_scalar("SELECT working_dir FROM sessions WHERE id = ?")
                .bind(&s1)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(wd.is_none(), "033 rows merge with NULL working_dir");

        // turn_count recounted on the new sessions.
        let tc: i64 = sqlx::query_scalar("SELECT turn_count FROM sessions WHERE id = ?")
            .bind(&s1)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(tc, 2);
        pool.close().await;

        // Shadow quarantined (renamed), backup exists, original gone.
        assert!(!shadow.exists(), "shadow must be renamed after commit");
        assert_eq!(report.quarantined_paths.len(), 1);
        assert!(report.quarantined_paths[0].exists());
        assert!(
            report.backups.iter().any(|b| b.exists()),
            "checkpoint-then-copy backups must exist"
        );

        // The quarantined shadow file got the introspection upgrade in place.
        let qpool = create_pool(&report.quarantined_paths[0].to_string_lossy())
            .await
            .unwrap();
        let cols = sqlx::query("PRAGMA table_info(sessions)")
            .fetch_all(&qpool)
            .await
            .unwrap();
        assert!(
            cols.iter().any(|r| r.get::<String, _>("name") == "working_dir"),
            "shadow sessions gained working_dir via targeted ALTER"
        );
        qpool.close().await;
    }

    #[tokio::test]
    async fn unify_dedups_by_tool_external_id_and_collapses_identical_turns() {
        let tmp = TempDir::new().unwrap();
        let canonical = tmp.path().join("canonical.db");
        let shadow = tmp.path().join("shadow.db");

        let cpool = fresh_db(&canonical).await;
        let cs = uid();
        seed_session(&cpool, &cs, "claude-code", Some("ext-A"), "2026-06-01T09:00:00.000Z", true)
            .await;
        seed_turn(&cpool, &uid(), &cs, 0, "same content", true).await;
        cpool.close().await;

        // Shadow: SAME (tool, external_id) under a DIFFERENT session id, with
        // one identical turn + one extra turn → merge + collapse + insert.
        let spool = fresh_db(&shadow).await;
        let ss = uid();
        seed_session(&spool, &ss, "claude-code", Some("ext-A"), "2026-06-01T09:00:00.000Z", true)
            .await;
        seed_turn(&spool, &uid(), &ss, 0, "same content", true).await;
        seed_turn(&spool, &uid(), &ss, 1, "extra shadow turn", true).await;
        spool.close().await;

        let report = unify(&UnifyOptions {
            canonical: canonical.clone(),
            shadows: vec![shadow],
            apply: true,
            backup_root: tmp.path().join("backups"),
        })
        .await
        .unwrap();

        assert_eq!(report.sessions_merged, 1);
        assert_eq!(report.sessions_new, 0);
        assert_eq!(report.turns_collapsed, 1, "identical turn collapses");
        assert_eq!(report.turns_inserted, 1, "the extra turn merges in");

        let pool = fresh_db(&canonical).await;
        assert_eq!(count(&pool, "sessions").await, 1, "no duplicate session");
        assert_eq!(count(&pool, "turns").await, 2);
        // FK remap: the inserted turn hangs off the CANONICAL session id.
        let sid: String = sqlx::query_scalar("SELECT session_id FROM turns WHERE turn_idx = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(sid, cs, "turn FK remapped to the kept session");
        let tc: i64 = sqlx::query_scalar("SELECT turn_count FROM sessions WHERE id = ?")
            .bind(&cs)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(tc, 2, "turn_count recounted after merge");
        pool.close().await;
    }

    #[tokio::test]
    async fn unify_merges_null_ext_by_turn_seq_hash_and_remaps_refs() {
        let tmp = TempDir::new().unwrap();
        let canonical = tmp.path().join("canonical.db");
        let shadow = tmp.path().join("shadow.db");

        let cpool = fresh_db(&canonical).await;
        let cs = uid();
        seed_session(&cpool, &cs, "claude-code", None, "2026-06-01T09:00:00.000Z", true).await;
        seed_turn(&cpool, &uid(), &cs, 0, "alpha", true).await;
        seed_turn(&cpool, &uid(), &cs, 1, "beta", true).await;
        cpool.close().await;

        // Shadow: NULL external_id, different session id, IDENTICAL full turn
        // sequence → auto-merge. Its improvement_signal + event must remap.
        let spool = fresh_db(&shadow).await;
        let ss = uid();
        seed_session(&spool, &ss, "claude-code", None, "2026-06-01T09:00:05.000Z", true).await;
        seed_turn(&spool, &uid(), &ss, 0, "alpha", true).await;
        seed_turn(&spool, &uid(), &ss, 1, "beta", true).await;
        sqlx::query(
            "INSERT INTO improvement_signals (id, kind, source_ref, summary) \
             VALUES ('sig-1', 'session_ingest', ?, 'shadow signal')",
        )
        .bind(format!("session:{ss}"))
        .execute(&spool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO events (id, event_type, actor_type, source, entity_type, entity_id, title) \
             VALUES ('ev-1', 'session_started', 'system', 'hook_handle', 'session', ?, 'started')",
        )
        .bind(&ss)
        .execute(&spool)
        .await
        .unwrap();
        spool.close().await;

        let report = unify(&UnifyOptions {
            canonical: canonical.clone(),
            shadows: vec![shadow],
            apply: true,
            backup_root: tmp.path().join("backups"),
        })
        .await
        .unwrap();

        assert_eq!(report.sessions_merged, 1, "seq-hash match auto-merges");
        assert_eq!(report.turns_collapsed, 2);
        assert_eq!(report.turns_inserted, 0);

        let pool = fresh_db(&canonical).await;
        assert_eq!(count(&pool, "sessions").await, 1);
        assert_eq!(count(&pool, "turns").await, 2);
        // FK remap on improvement_signals payload ref + events.entity_id.
        let sref: String =
            sqlx::query_scalar("SELECT source_ref FROM improvement_signals WHERE id = 'sig-1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(sref, format!("session:{cs}"), "signal ref remapped");
        let eid: String = sqlx::query_scalar("SELECT entity_id FROM events WHERE id = 'ev-1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(eid, cs, "event entity_id remapped");
        pool.close().await;
    }

    #[tokio::test]
    async fn unify_quarantines_ambiguous_null_ext_session() {
        let tmp = TempDir::new().unwrap();
        let canonical = tmp.path().join("canonical.db");
        let shadow = tmp.path().join("shadow.db");

        let cpool = fresh_db(&canonical).await;
        let cs = uid();
        seed_session(&cpool, &cs, "claude-code", None, "2026-06-01T09:00:00.000Z", true).await;
        seed_turn(&cpool, &uid(), &cs, 0, "alpha", true).await;
        seed_turn(&cpool, &uid(), &cs, 1, "beta", true).await;
        seed_turn(&cpool, &uid(), &cs, 2, "gamma", true).await;
        cpool.close().await;

        // Shadow session: NULL ext, different id, turn sequence is a strict
        // PREFIX of the canonical one — partial match → quarantine, never merge.
        let spool = fresh_db(&shadow).await;
        let ss = uid();
        seed_session(&spool, &ss, "claude-code", None, "2026-06-01T09:00:30.000Z", true).await;
        seed_turn(&spool, &uid(), &ss, 0, "alpha", true).await;
        seed_turn(&spool, &uid(), &ss, 1, "beta", true).await;
        spool.close().await;

        let report = unify(&UnifyOptions {
            canonical: canonical.clone(),
            shadows: vec![shadow],
            apply: true,
            backup_root: tmp.path().join("backups"),
        })
        .await
        .unwrap();

        assert_eq!(report.sessions_quarantined, 1, "partial match quarantines");
        assert_eq!(report.sessions_merged, 0);
        assert_eq!(report.sessions_new, 0);
        assert!(
            report.conflicts.iter().any(|c| c.contains("QUARANTINED")),
            "conflict report names the quarantined session: {:?}",
            report.conflicts
        );

        let pool = fresh_db(&canonical).await;
        assert_eq!(count(&pool, "sessions").await, 1, "canonical untouched");
        assert_eq!(count(&pool, "turns").await, 3);
        pool.close().await;
    }

    #[tokio::test]
    async fn divergent_turn_collision_goes_to_turns_quarantine() {
        let tmp = TempDir::new().unwrap();
        let canonical = tmp.path().join("canonical.db");
        let shadow = tmp.path().join("shadow.db");

        let cpool = fresh_db(&canonical).await;
        let sid = uid();
        seed_session(&cpool, &sid, "claude-code", None, "2026-06-01T09:00:00.000Z", true).await;
        seed_turn(&cpool, &uid(), &sid, 0, "original content", true).await;
        cpool.close().await;

        // Shadow: SAME session id (id-match merge) but turn 0 DIVERGES.
        let spool = fresh_db(&shadow).await;
        seed_session(&spool, &sid, "claude-code", None, "2026-06-01T09:00:00.000Z", true).await;
        seed_turn(&spool, &uid(), &sid, 0, "FORKED divergent content", true).await;
        spool.close().await;

        let report = unify(&UnifyOptions {
            canonical: canonical.clone(),
            shadows: vec![shadow],
            apply: true,
            backup_root: tmp.path().join("backups"),
        })
        .await
        .unwrap();

        assert_eq!(report.turns_quarantined, 1);
        assert_eq!(report.turns_inserted, 0);

        let pool = fresh_db(&canonical).await;
        // Canonical turn NEVER overwritten; UNIQUE(session_id, turn_idx) intact.
        let content: String =
            sqlx::query_scalar("SELECT content FROM turns WHERE session_id = ? AND turn_idx = 0")
                .bind(&sid)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(content, "original content", "canonical turn untouched");
        let qrow = sqlx::query(
            "SELECT content, reason FROM turns_quarantine WHERE session_id = ? AND turn_idx = 0",
        )
        .bind(&sid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            qrow.get::<String, _>("content"),
            "FORKED divergent content"
        );
        assert_eq!(qrow.get::<String, _>("reason"), "divergent_turn_idx");
        pool.close().await;
    }

    #[tokio::test]
    async fn merged_content_tables_are_fts_findable() {
        let tmp = TempDir::new().unwrap();
        let canonical = tmp.path().join("canonical.db");
        let shadow = tmp.path().join("shadow.db");

        fresh_db(&canonical).await.close().await;

        // Shadow holds a learning (LearningsRepository writes learnings +
        // object_index + object_fts together — the real shadow's shape).
        let spool = fresh_db(&shadow).await;
        {
            let learnings = altevra_db::LearningsRepository::new(&spool);
            let row = altevra_db::LearningRow::new(
                "L-shadow-1",
                "Shadow GTM learning",
                "Florida surplus buyers respond to direct mail",
            );
            learnings.insert(&row).await.unwrap();
        }
        spool.close().await;

        let report = unify(&UnifyOptions {
            canonical: canonical.clone(),
            shadows: vec![shadow],
            apply: true,
            backup_root: tmp.path().join("backups"),
        })
        .await
        .unwrap();
        assert!(report.fts_reindexed >= 1, "merged object was FTS re-indexed");
        assert_eq!(report.content.get("learnings").unwrap().inserted, 1);

        let pool = fresh_db(&canonical).await;
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM learnings WHERE id = 'L-shadow-1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1, "learning row merged");
        // The unify gate: merged content is findable through FTS.
        let fts = FtsRepository::new(&pool);
        let hits = fts.search("Florida surplus", 10).await.unwrap();
        assert!(
            hits.iter().any(|h| h.object_id == "L-shadow-1"),
            "merged learning must be FTS-findable, got: {hits:?}"
        );
        pool.close().await;
    }

    #[tokio::test]
    async fn merged_shadow_turns_are_reguarded() {
        let tmp = TempDir::new().unwrap();
        let canonical = tmp.path().join("canonical.db");
        let shadow = tmp.path().join("shadow.db");
        fresh_db(&canonical).await.close().await;

        // Shadow turn smuggles a raw secret while CLAIMING redaction 'clean'
        // (033-era weaker redaction). Unify must re-guard on insert.
        let spool = fresh_db(&shadow).await;
        let ss = uid();
        seed_session(&spool, &ss, "claude-code", None, "2026-06-01T09:00:00.000Z", true).await;
        seed_turn(
            &spool,
            &uid(),
            &ss,
            0,
            "export OPENAI_API_KEY=sk-ant-AAAAAAAAAAAAAAAAAAAAAAAA done",
            true,
        )
        .await;
        spool.close().await;

        unify(&UnifyOptions {
            canonical: canonical.clone(),
            shadows: vec![shadow],
            apply: true,
            backup_root: tmp.path().join("backups"),
        })
        .await
        .unwrap();

        let pool = fresh_db(&canonical).await;
        let row = sqlx::query("SELECT content, redaction_status, redacted_count FROM turns")
            .fetch_one(&pool)
            .await
            .unwrap();
        let content: String = row.get("content");
        assert!(
            !content.contains("sk-ant-AAAAAAAAAAAAAAAAAAAAAAAA"),
            "secret must be re-guarded out of merged content: {content}"
        );
        assert_eq!(row.get::<String, _>("redaction_status"), "redacted");
        assert!(row.get::<i64, _>("redacted_count") >= 1);
        pool.close().await;
    }

    #[tokio::test]
    async fn dry_run_changes_nothing_but_reports_exact_counts() {
        let tmp = TempDir::new().unwrap();
        let canonical = tmp.path().join("canonical.db");
        let shadow = tmp.path().join("shadow.db");

        let cpool = fresh_db(&canonical).await;
        let cs = uid();
        seed_session(&cpool, &cs, "claude-code", None, "2026-06-01T09:00:00.000Z", true).await;
        seed_turn(&cpool, &uid(), &cs, 0, "canon", true).await;
        cpool.close().await;

        // 033-schema shadow — dry-run must NOT upgrade the original either.
        let spool = make_shadow_033(&shadow).await;
        let ss = uid();
        seed_session(&spool, &ss, "codex", None, "2026-06-02T09:00:00.000Z", false).await;
        seed_turn(&spool, &uid(), &ss, 0, "shadow", false).await;
        spool.close().await;

        let report = unify(&UnifyOptions {
            canonical: canonical.clone(),
            shadows: vec![shadow.clone()],
            apply: false,
            backup_root: tmp.path().join("backups"),
        })
        .await
        .unwrap();

        // Dry-run report shows the EXACT planned after-counts…
        assert_eq!(report.counts_before.get("sessions"), Some(&1));
        assert_eq!(report.counts_after.get("sessions"), Some(&2));
        assert_eq!(report.counts_before.get("turns"), Some(&1));
        assert_eq!(report.counts_after.get("turns"), Some(&2));
        assert!(!report.applied);

        // …but NOTHING changed on disk.
        assert!(shadow.exists(), "shadow not renamed in dry-run");
        assert!(report.quarantined_paths.is_empty());
        assert!(report.backups.is_empty(), "no backups in dry-run");
        let pool = fresh_db(&canonical).await;
        assert_eq!(count(&pool, "sessions").await, 1, "canonical untouched");
        assert_eq!(count(&pool, "turns").await, 1);
        pool.close().await;
        // Original shadow keeps its 033 schema (only the temp COPY upgraded).
        let qpool = create_pool(&shadow.to_string_lossy()).await.unwrap();
        let cols = sqlx::query("PRAGMA table_info(sessions)")
            .fetch_all(&qpool)
            .await
            .unwrap();
        assert!(
            !cols.iter().any(|r| r.get::<String, _>("name") == "working_dir"),
            "dry-run must not ALTER the original shadow"
        );
        qpool.close().await;
    }

    // -----------------------------------------------------------------------
    // Spool: write / redaction / perms / replay-by-id / idempotency
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn replay_spool_ingests_by_id_and_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let spool = tmp.path().join("spool");
        let pool = fresh_db(&tmp.path().join("db.db")).await;

        let sid = Uuid::new_v4();
        let start = SpoolEntry::SessionStart {
            tool: "claude-code".into(),
            session_id: sid,
            project_name: Some("altevra".into()),
            started_at: Utc::now(),
            working_dir: Some("/home/pavle/projekti/ai-tooling/altevra".into()),
        };
        let t1 = build_spool_turn(
            "claude-code",
            sid,
            "user_prompt_submit",
            &serde_json::json!({"user_prompt": "spooled prompt one"}),
            None,
        )
        .unwrap();
        let t2 = build_spool_turn(
            "claude-code",
            sid,
            "post_tool_use",
            &serde_json::json!({"tool_name": "Bash", "tool_response": "ok"}),
            None,
        )
        .unwrap();
        let end = SpoolEntry::SessionEnd {
            tool: "claude-code".into(),
            session_id: sid,
            summary: Some("done".into()),
            ended_at: Utc::now(),
        };
        write_spool_entry(&spool, "claude-code", &start).unwrap();
        write_spool_entry(&spool, "claude-code", &t1).unwrap();
        let t2_path = write_spool_entry(&spool, "claude-code", &t2).unwrap();
        write_spool_entry(&spool, "claude-code", &end).unwrap();

        let report = replay_spool_dir(&pool, &spool).await.unwrap();
        assert_eq!(report.replayed, 4);
        assert_eq!(report.failed, 0);
        assert_eq!(
            std::fs::read_dir(&spool).unwrap().count(),
            0,
            "successful replay removes the files"
        );

        // Session + 2 turns landed under the embedded ids (direct-by-id).
        let sess = SessionsRepository::new(&pool)
            .get_session(sid)
            .await
            .unwrap()
            .unwrap();
        assert!(sess.ended_at.is_some(), "session_end replayed");
        assert_eq!(sess.summary.as_deref(), Some("done"));
        let turns = SessionsRepository::new(&pool)
            .list_turns(sid, 10)
            .await
            .unwrap();
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].turn_idx, 0);
        assert_eq!(turns[1].turn_idx, 1);
        assert_eq!(turns[1].role, "tool_result");

        // Idempotency: re-spooling the SAME turn id and replaying again does
        // not duplicate (replay-after-partial-failure case).
        std::fs::write(&t2_path, serde_json::to_vec(&t2).unwrap()).unwrap();
        let report = replay_spool_dir(&pool, &spool).await.unwrap();
        assert_eq!(report.replayed, 1);
        assert_eq!(report.failed, 0);
        assert_eq!(
            SessionsRepository::new(&pool)
                .list_turns(sid, 10)
                .await
                .unwrap()
                .len(),
            2,
            "replay re-run is idempotent (stable turn id)"
        );
        pool.close().await;
    }

    #[tokio::test]
    async fn replay_handles_session_ended_during_unify() {
        // The session started BEFORE the lock (it exists in canonical with a
        // live pointer); the END + a final turn arrive while unify holds the
        // lock → both spool, both replay by id against the pre-existing row.
        let tmp = TempDir::new().unwrap();
        let spool = tmp.path().join("spool");
        let pool = fresh_db(&tmp.path().join("db.db")).await;

        let sid = Uuid::new_v4();
        SessionsRepository::new(&pool)
            .start_session(&SessionRow {
                id: sid,
                tool: "claude-code".into(),
                project_id: None,
                project_name: Some("altevra".into()),
                started_at: Utc::now(),
                ended_at: None,
                summary: None,
                tokens_in_total: 0,
                tokens_out_total: 0,
                cost_usd_estimate: 0.0,
                turn_count: 0,
                metadata: serde_json::json!({}),
                external_id: None,
                imported_from: None,
                working_dir: None,
            })
            .await
            .unwrap();

        let turn = build_spool_turn(
            "claude-code",
            sid,
            "user_prompt_submit",
            &serde_json::json!({"user_prompt": "last words before unify"}),
            None,
        )
        .unwrap();
        write_spool_entry(&spool, "claude-code", &turn).unwrap();
        write_spool_entry(
            &spool,
            "claude-code",
            &SpoolEntry::SessionEnd {
                tool: "claude-code".into(),
                session_id: sid,
                summary: None,
                ended_at: Utc::now(),
            },
        )
        .unwrap();

        let report = replay_spool_dir(&pool, &spool).await.unwrap();
        assert_eq!(report.replayed, 2);
        assert_eq!(report.failed, 0);
        let sess = SessionsRepository::new(&pool)
            .get_session(sid)
            .await
            .unwrap()
            .unwrap();
        assert!(sess.ended_at.is_some(), "spooled session_end closed it");
        assert_eq!(sess.turn_count, 1, "spooled turn ingested under its id");
        pool.close().await;
    }

    #[tokio::test]
    async fn replay_failure_keeps_file_and_writes_audit_row() {
        let tmp = TempDir::new().unwrap();
        let spool = tmp.path().join("spool");
        let pool = fresh_db(&tmp.path().join("db.db")).await;

        // A turn for a session that exists NOWHERE → loud failure.
        let orphan = build_spool_turn(
            "claude-code",
            Uuid::new_v4(),
            "user_prompt_submit",
            &serde_json::json!({"user_prompt": "orphan"}),
            None,
        )
        .unwrap();
        let path = write_spool_entry(&spool, "claude-code", &orphan).unwrap();

        let report = replay_spool_dir(&pool, &spool).await.unwrap();
        assert_eq!(report.replayed, 0);
        assert_eq!(report.failed, 1);
        assert!(path.exists(), "failed replay keeps the spool file");

        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM audit_log WHERE action = 'spool_replay_failed'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(n, 1, "failure writes an audit_log row");
        pool.close().await;
    }

    #[test]
    fn spool_payload_is_redacted_before_disk_and_file_is_0600() {
        let tmp = TempDir::new().unwrap();
        let spool = tmp.path().join("spool");
        let sid = Uuid::new_v4();
        let secret = "sk-ant-AAAAAAAAAAAAAAAAAAAAAAAA";
        let entry = build_spool_turn(
            "claude-code",
            sid,
            "post_tool_use",
            &serde_json::json!({
                "tool_name": "Bash",
                "tool_response": format!("export OPENAI_API_KEY={secret}"),
                "tool_input": {"command": format!("echo {secret} to alice@example.com")},
            }),
            None,
        )
        .unwrap();
        let path = write_spool_entry(&spool, "claude-code", &entry).unwrap();

        let bytes = std::fs::read_to_string(&path).unwrap();
        assert!(
            !bytes.contains(secret),
            "raw secret must NEVER reach the spool file: {bytes}"
        );
        assert!(
            !bytes.contains("alice@example.com"),
            "PII must be redacted before disk"
        );
        assert!(
            bytes.contains(&sid.to_string()),
            "spool entry embeds the session_id"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "spool file must be created 0600");
        }
    }

    #[test]
    fn spool_filenames_are_unique_per_event() {
        let tmp = TempDir::new().unwrap();
        let spool = tmp.path().join("spool");
        let sid = Uuid::new_v4();
        let entry = build_spool_turn(
            "claude-code",
            sid,
            "user_prompt_submit",
            &serde_json::json!({"user_prompt": "x"}),
            None,
        )
        .unwrap();
        // Many rapid writes from the same pid → all land as distinct files
        // (O_EXCL + nanosecond bump on collision).
        let mut paths = HashSet::new();
        for _ in 0..10 {
            paths.insert(write_spool_entry(&spool, "claude-code", &entry).unwrap());
        }
        assert_eq!(paths.len(), 10, "one file per event, never overwritten");
    }

    // -----------------------------------------------------------------------
    // Discovery
    // -----------------------------------------------------------------------

    #[test]
    fn discovery_finds_dot_altevra_shadows_and_excludes_canonical() {
        let tmp = TempDir::new().unwrap();
        let canonical = tmp.path().join("home/.altevra/altevra.db");
        std::fs::create_dir_all(canonical.parent().unwrap()).unwrap();
        std::fs::write(&canonical, b"x").unwrap();

        let shadow = tmp.path().join("projekti/repo/.altevra/altevra.db");
        std::fs::create_dir_all(shadow.parent().unwrap()).unwrap();
        std::fs::write(&shadow, b"x").unwrap();

        // A non-.altevra altevra.db must NOT be picked up.
        let decoy = tmp.path().join("projekti/other/altevra.db");
        std::fs::create_dir_all(decoy.parent().unwrap()).unwrap();
        std::fs::write(&decoy, b"x").unwrap();

        let found = discover_shadows(
            &canonical,
            &[canonical.clone()], // explicit canonical must be excluded
            &[tmp.path().join("projekti")],
        );
        assert_eq!(found.len(), 1, "exactly the .altevra shadow: {found:?}");
        assert_eq!(found[0], shadow.canonicalize().unwrap());
    }
}

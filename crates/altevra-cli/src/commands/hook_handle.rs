//! `altevra hook handle <event-name>` — parse a tool-emitted hook payload
//! from stdin and translate it into a `turn record` row.
//!
//! Used by Claude Code / Codex / Cursor / Antigravity hook configs that pipe
//! the tool's hook event JSON into Altevra. Schema per tool:
//!
//! * Claude Code `UserPromptSubmit`: `{"user_prompt": "..."}`
//! * Claude Code `PostToolUse`: `{"tool_name": "...", "tool_input": {...}, "tool_response": "..."}`
//! * Claude Code `SessionStart`: emits a new session id and writes the
//!   current-session pointer.
//! * Claude Code `Stop` / `SessionEnd`: closes the current session.
//!
//! Codex/Cursor schemas are similar enough — we read common fields (`prompt`,
//! `tool_name`, `command`, `content`) defensively.

use altevra_db::{
    create_pool, run_migrations, signal_for_session, ImprovementSignalsRepository, SessionRow,
    SessionsRepository, TurnRow,
};
use altevra_secrets::{auto_capture, guard_text, SecretStore};
use chrono::Utc;
use clap::Args;
use std::io::Read;
use std::path::{Path, PathBuf};
use uuid::Uuid;

const CURRENT_SESSION_FILE: &str = ".altevra/state/current_session.txt";

#[derive(Args)]
pub struct HookHandleArgs {
    /// Event name (e.g. user_prompt_submit, post_tool_use, session_start, session_end).
    pub event: String,

    /// Tool emitting the event (defaults to claude-code).
    #[arg(long, default_value = "claude-code")]
    pub tool: String,

    /// Project name override.
    #[arg(long)]
    pub project: Option<String>,

    /// SQLite database path.
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,

    /// Skip stdin reading (for tests).
    #[arg(long)]
    pub no_stdin: bool,
}

pub async fn run(args: HookHandleArgs) -> anyhow::Result<()> {
    // Read stdin payload (tool hook system pipes JSON in).
    let payload = if args.no_stdin {
        serde_json::json!({})
    } else {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf).ok();
        serde_json::from_str::<serde_json::Value>(&buf)
            .unwrap_or(serde_json::Value::Object(Default::default()))
    };

    let pool = create_pool(&args.db.to_string_lossy()).await?;
    run_migrations(&pool).await?;
    let repo = SessionsRepository::new(&pool);

    match args.event.as_str() {
        "session_start" => handle_session_start(&repo, &args, &payload).await?,
        "session_end" | "stop" => handle_session_end(&pool, &repo, &args, &payload).await?,
        "user_prompt_submit" => handle_user_prompt(&repo, &args, &payload).await?,
        "post_tool_use" => handle_post_tool_use(&repo, &args, &payload).await?,
        "pre_tool_use" => handle_pre_tool_use(&repo, &args, &payload).await?,
        other => {
            eprintln!("[altevra] unknown hook event: {other}");
        }
    }
    Ok(())
}

async fn handle_session_start(
    repo: &SessionsRepository<'_>,
    args: &HookHandleArgs,
    _payload: &serde_json::Value,
) -> anyhow::Result<()> {
    let row = SessionRow {
        id: Uuid::new_v4(),
        tool: args.tool.clone(),
        project_id: None,
        project_name: args.project.clone(),
        started_at: Utc::now(),
        ended_at: None,
        summary: None,
        tokens_in_total: 0,
        tokens_out_total: 0,
        cost_usd_estimate: 0.0,
        turn_count: 0,
        metadata: serde_json::json!({"started_via": "hook"}),
        external_id: None,
        imported_from: None,
    };
    repo.start_session(&row).await?;
    write_current_session(&row.id)?;
    println!("{{\"session_id\":\"{}\"}}", row.id);
    Ok(())
}

async fn handle_session_end(
    pool: &sqlx::SqlitePool,
    repo: &SessionsRepository<'_>,
    _args: &HookHandleArgs,
    payload: &serde_json::Value,
) -> anyhow::Result<()> {
    let summary = payload
        .get("summary")
        .and_then(serde_json::Value::as_str)
        .map(String::from);
    if let Some(id) = read_current_session()? {
        repo.end_session(id, summary.as_deref()).await?;
        // Real-time self-improve producer (C1): one cheap improvement_signal per
        // session ingest — the orchestrator (a later seam) clusters open signals
        // into proposals. SI-6 self-write exclusion: a resident-mode-authored
        // session enqueues NOTHING (signal_for_session returns None), so
        // Altevra's own output never feeds the loop back into itself. Best-effort:
        // a signal-enqueue failure must NOT block closing the session.
        enqueue_session_signal(pool, repo, id).await;
        clear_current_session()?;
        println!("{{\"closed_session\":\"{id}\"}}");
    }
    Ok(())
}

/// Enqueue the per-session improvement signal (C1 producer). Reads the closed
/// session's provenance (`tool`/`project`/`turn_count`) and asks the pure
/// [`signal_for_session`] producer for a signal — which is `None` when SI-6
/// excludes a resident-authored session. Best-effort: any error is logged to
/// stderr and swallowed so the hook never fails the agent's session close.
async fn enqueue_session_signal(pool: &sqlx::SqlitePool, repo: &SessionsRepository<'_>, id: Uuid) {
    let session = match repo.get_session(id).await {
        Ok(Some(s)) => s,
        Ok(None) => return,
        Err(e) => {
            eprintln!("[altevra] signal enqueue skipped (session lookup failed): {e}");
            return;
        }
    };
    // SI-6 is enforced inside signal_for_session: resident-authored → None.
    let Some(new_signal) =
        signal_for_session(&id.to_string(), &session.tool, session.project_name.as_deref(), session.turn_count)
    else {
        return;
    };
    let signals = ImprovementSignalsRepository::new(pool);
    if let Err(e) = signals.insert(&new_signal).await {
        eprintln!("[altevra] improvement_signal enqueue failed (non-fatal): {e}");
    }
}

async fn handle_user_prompt(
    repo: &SessionsRepository<'_>,
    args: &HookHandleArgs,
    payload: &serde_json::Value,
) -> anyhow::Result<()> {
    let content =
        first_field(payload, &["user_prompt", "prompt", "content", "message"]).unwrap_or_default();
    record_turn(repo, "user", &content, None, &args.tool, payload).await
}

async fn handle_pre_tool_use(
    repo: &SessionsRepository<'_>,
    args: &HookHandleArgs,
    payload: &serde_json::Value,
) -> anyhow::Result<()> {
    let tool_name = payload
        .get("tool_name")
        .and_then(serde_json::Value::as_str)
        .map(String::from);
    let content = payload
        .get("tool_input")
        .map(|v| v.to_string())
        .unwrap_or_default();
    record_turn(repo, "tool_call", &content, tool_name, &args.tool, payload).await
}

async fn handle_post_tool_use(
    repo: &SessionsRepository<'_>,
    args: &HookHandleArgs,
    payload: &serde_json::Value,
) -> anyhow::Result<()> {
    let tool_name = payload
        .get("tool_name")
        .and_then(serde_json::Value::as_str)
        .map(String::from);
    let response = first_field(payload, &["tool_response", "result", "output"]).unwrap_or_default();
    record_turn(
        repo,
        "tool_result",
        &response,
        tool_name,
        &args.tool,
        payload,
    )
    .await
}

/// Recursively scrub every string leaf of a JSON value through `guard_text`.
/// Tool inputs and file diffs are the richest source of raw secrets/PII (Edit
/// payloads, `export OPENAI_API_KEY=...`, customer emails in file contents), so
/// they must be scrubbed before persistence (R11 #2 — content-only redaction
/// left these sibling columns raw). Returns the scrubbed value, the count of
/// redacted leaves, and the worst-case sensitivity across all leaves.
pub(crate) fn guard_json(
    v: &serde_json::Value,
) -> (serde_json::Value, i64, altevra_core::security::Sensitivity) {
    use altevra_core::security::Sensitivity;
    use altevra_core::status::RedactionStatus;
    match v {
        serde_json::Value::String(s) => {
            let g = guard_text(s, Sensitivity::Internal);
            let n = i64::from(matches!(g.redaction_status, RedactionStatus::Redacted));
            (serde_json::Value::String(g.value), n, g.sensitivity)
        }
        serde_json::Value::Array(arr) => {
            let mut out = Vec::with_capacity(arr.len());
            let mut count = 0i64;
            let mut sens = Sensitivity::Internal;
            for item in arr {
                let (vv, c, sn) = guard_json(item);
                out.push(vv);
                count += c;
                sens = sens.combine(&sn);
            }
            (serde_json::Value::Array(out), count, sens)
        }
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            let mut count = 0i64;
            let mut sens = Sensitivity::Internal;
            for (k, vv) in map {
                let (rv, c, sn) = guard_json(vv);
                out.insert(k.clone(), rv);
                count += c;
                sens = sens.combine(&sn);
            }
            (serde_json::Value::Object(out), count, sens)
        }
        other => (other.clone(), 0, Sensitivity::Internal),
    }
}

async fn record_turn(
    repo: &SessionsRepository<'_>,
    role: &str,
    raw_content: &str,
    tool_name: Option<String>,
    source_tool: &str,
    payload: &serde_json::Value,
) -> anyhow::Result<()> {
    let session_id = match read_current_session()? {
        Some(id) => id,
        None => {
            // No active session — silently skip so the agent isn't blocked
            // when running outside the recorded lifecycle.
            return Ok(());
        }
    };

    // Auto-capture: any detected secret is persisted into the secret store
    // under a stable key (fingerprint-based) BEFORE the content gets redacted
    // out of chat history. This is Pavle's "don't make me re-send keys"
    // feature — Altevra grabs them once, stores forever, then redacts.
    let store = resolve_capture_store();
    let captures = auto_capture(raw_content, &store).unwrap_or_default();

    // PreWriteSafetyGate text path (T1.13): scrub BOTH secrets AND PII (emails)
    // and classify sensitivity before the turn is persisted. Previously only
    // secrets were redacted; PII leaked into stored content. guard_text raises
    // sensitivity (default-up) when credential/PII risk is present.
    let guarded = guard_text(raw_content, altevra_core::Sensitivity::Internal);
    let final_content = guarded.value;
    // redacted_count reflects everything scrubbed: captured secrets, plus a PII
    // bump when emails were redacted.
    let mut redacted_count = captures.len().max(guarded.sightings.len()) as i64;
    if guarded
        .risk_tags
        .contains(&altevra_core::RiskTag::ThirdPartyPii)
    {
        redacted_count += 1;
    }
    let mut turn_sensitivity = guarded.sensitivity.clone();
    let mut turn_redaction = guarded.redaction_status.clone();

    // R11 #2: scrub the side-channel columns (tool_input + file_changes) too —
    // never persist them raw. Worst-case sensitivity is folded into the turn.
    let guarded_tool_input = payload.get("tool_input").map(guard_json);
    let guarded_file_changes = payload.get("file_changes").map(guard_json);
    let mut side_channel_redacted = false;
    for g in [&guarded_tool_input, &guarded_file_changes]
        .into_iter()
        .flatten()
    {
        redacted_count += g.1;
        turn_sensitivity = turn_sensitivity.combine(&g.2);
        side_channel_redacted |= g.1 > 0;
    }
    if side_channel_redacted {
        turn_redaction = altevra_core::status::RedactionStatus::Redacted;
    }
    let scrubbed_tool_input = guarded_tool_input.map(|(v, _, _)| v);
    let scrubbed_file_changes = guarded_file_changes.map(|(v, _, _)| v);

    let capture_meta: Vec<serde_json::Value> = captures
        .iter()
        .map(|c| {
            serde_json::json!({
                "kind": format!("{:?}", c.kind).to_lowercase(),
                "key": c.key,
                "fingerprint": c.fingerprint,
                "was_new": c.was_new,
            })
        })
        .collect();

    let turn_idx = repo.next_turn_idx(session_id).await?;
    let turn = TurnRow {
        id: Uuid::new_v4(),
        session_id,
        turn_idx,
        role: role.to_string(),
        content: final_content,
        tool_calls: if capture_meta.is_empty() {
            scrubbed_tool_input.clone()
        } else {
            Some(serde_json::json!({
                "tool_input": scrubbed_tool_input.clone(),
                "captured_secrets": capture_meta,
            }))
        },
        tool_name,
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
        file_changes: scrubbed_file_changes,
        redacted_count,
        source_tool: Some(source_tool.to_string()),
        sensitivity: turn_sensitivity.to_string(),
        redaction_status: turn_redaction.to_string(),
        created_at: Utc::now(),
    };
    repo.record_turn(&turn).await?;
    Ok(())
}

/// Resolve which SecretStore backend auto-capture should use.
///
/// Priority:
///   1. `ALTEVRA_SECRETS_FILE` env var → encrypted file at that path.
///   2. `~/.altevra/secrets.enc` if `ALTEVRA_SECRETS_KEY` env is set.
///   3. OS keyring (default).
fn resolve_capture_store() -> SecretStore {
    if let Ok(path) = std::env::var("ALTEVRA_SECRETS_FILE") {
        let key_env = std::env::var("ALTEVRA_SECRETS_KEY_ENV")
            .unwrap_or_else(|_| "ALTEVRA_SECRETS_KEY".into());
        return SecretStore::new_encrypted_file("altevra", path.into(), &key_env);
    }
    if std::env::var("ALTEVRA_SECRETS_KEY").is_ok() {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        let path = std::path::PathBuf::from(home).join(".altevra/secrets.enc");
        return SecretStore::new_encrypted_file("altevra", path, "ALTEVRA_SECRETS_KEY");
    }
    SecretStore::new_keyring("altevra")
}

fn first_field(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Some(s) = value.get(*k).and_then(serde_json::Value::as_str) {
            return Some(s.to_string());
        }
    }
    None
}

fn write_current_session(id: &Uuid) -> anyhow::Result<()> {
    let path = Path::new(CURRENT_SESSION_FILE);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, id.to_string())?;
    Ok(())
}

fn read_current_session() -> anyhow::Result<Option<Uuid>> {
    let path = Path::new(CURRENT_SESSION_FILE);
    if !path.exists() {
        return Ok(None);
    }
    let s = std::fs::read_to_string(path)?;
    Ok(Uuid::parse_str(s.trim()).ok())
}

fn clear_current_session() -> anyhow::Result<()> {
    let path = Path::new(CURRENT_SESSION_FILE);
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Seed a closed session row with a given tool + turn count, returning its id.
    async fn seed_session(
        pool: &sqlx::SqlitePool,
        tool: &str,
        project: Option<&str>,
        turns: i64,
    ) -> Uuid {
        let repo = SessionsRepository::new(pool);
        let id = Uuid::new_v4();
        repo.start_session(&SessionRow {
            id,
            tool: tool.to_string(),
            project_id: None,
            project_name: project.map(String::from),
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
        })
        .await
        .unwrap();
        for i in 0..turns {
            repo.record_turn(&TurnRow {
                id: Uuid::new_v4(),
                session_id: id,
                turn_idx: i,
                role: "user".into(),
                content: format!("turn {i}"),
                tool_calls: None,
                tool_name: None,
                model: None,
                tokens_in: None,
                tokens_out: None,
                latency_ms: None,
                file_changes: None,
                redacted_count: 0,
                source_tool: Some(tool.to_string()),
                sensitivity: altevra_core::Sensitivity::Internal.to_string(),
                redaction_status: altevra_core::status::RedactionStatus::Clean.to_string(),
                created_at: Utc::now(),
            })
            .await
            .unwrap();
        }
        repo.end_session(id, None).await.unwrap();
        id
    }

    #[tokio::test]
    async fn session_ingest_enqueues_exactly_one_signal() {
        // C1 producer: a real external-tool session ingest enqueues exactly ONE
        // improvement_signal; running the producer again is idempotent (no 2nd row).
        let tmp = TempDir::new().unwrap();
        let pool = create_pool(&tmp.path().join("a.db").to_string_lossy())
            .await
            .unwrap();
        run_migrations(&pool).await.unwrap();
        let repo = SessionsRepository::new(&pool);
        let id = seed_session(&pool, "claude-code", Some("altevra"), 3).await;

        enqueue_session_signal(&pool, &repo, id).await;
        let signals = ImprovementSignalsRepository::new(&pool);
        let open = signals.list_open().await.unwrap();
        assert_eq!(open.len(), 1, "exactly one signal per session ingest");
        assert_eq!(open[0].kind, "session_ingest");
        assert_eq!(open[0].source_ref, format!("session:{id}"));
        assert_eq!(
            open[0].cluster_key.as_deref(),
            Some("session:claude-code:altevra")
        );

        // Idempotent: re-running the producer for the same session does NOT add a
        // second row (stable dedup id).
        enqueue_session_signal(&pool, &repo, id).await;
        assert_eq!(
            signals.list_open().await.unwrap().len(),
            1,
            "producer re-run is idempotent"
        );
    }

    #[tokio::test]
    async fn resident_authored_session_enqueues_no_signal_si6() {
        // SI-6: a session authored by a resident mode must NEVER become a signal
        // (no self-feedback loop). The producer enqueues ZERO rows for it.
        let tmp = TempDir::new().unwrap();
        let pool = create_pool(&tmp.path().join("b.db").to_string_lossy())
            .await
            .unwrap();
        run_migrations(&pool).await.unwrap();
        let repo = SessionsRepository::new(&pool);
        let id = seed_session(&pool, "resident:observer", None, 2).await;

        enqueue_session_signal(&pool, &repo, id).await;
        let signals = ImprovementSignalsRepository::new(&pool);
        assert_eq!(
            signals.list_open().await.unwrap().len(),
            0,
            "SI-6: resident-authored session enqueues nothing"
        );
    }

    #[tokio::test]
    async fn session_start_writes_pointer_file() {
        let tmp = TempDir::new().unwrap();
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let args = HookHandleArgs {
            event: "session_start".into(),
            tool: "claude-code".into(),
            project: Some("altevra".into()),
            db: tmp.path().join("altevra.db"),
            no_stdin: true,
        };
        run(args).await.unwrap();
        assert!(tmp.path().join(CURRENT_SESSION_FILE).exists());

        // session_end clears pointer
        let args = HookHandleArgs {
            event: "session_end".into(),
            tool: "claude-code".into(),
            project: None,
            db: tmp.path().join("altevra.db"),
            no_stdin: true,
        };
        run(args).await.unwrap();
        assert!(!tmp.path().join(CURRENT_SESSION_FILE).exists());

        std::env::set_current_dir(cwd).unwrap();
    }

    #[test]
    fn guard_json_scrubs_secret_and_pii_in_tool_input() {
        // R11 #2: tool_input/file_changes used to persist RAW. guard_json must
        // scrub every string leaf (secrets + PII) and raise sensitivity.
        let v = serde_json::json!({
            "command": "export OPENAI_API_KEY=sk-ant-AAAAAAAAAAAAAAAAAAAAAAAA",
            "edits": ["contact alice@example.com", 42, null],
        });
        let (scrubbed, count, sens) = guard_json(&v);
        let s = scrubbed.to_string();
        assert!(
            !s.contains("sk-ant-AAAAAAAAAAAAAAAAAAAAAAAA"),
            "secret leaked through tool_input: {s}"
        );
        assert!(!s.contains("alice@example.com"), "email leaked: {s}");
        assert!(count >= 2, "expected ≥2 redacted leaves, got {count}");
        assert!(sens >= altevra_core::security::Sensitivity::Confidential);
        // non-string leaves survive untouched.
        assert!(s.contains("42"));
    }
}

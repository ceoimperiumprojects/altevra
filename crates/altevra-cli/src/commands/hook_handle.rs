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

use altevra_db::{create_pool, run_migrations, SessionRow, SessionsRepository, TurnRow};
use altevra_secrets::{auto_capture, redact, SecretStore};
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
    #[arg(long, default_value = ".altevra/altevra.db")]
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
        "session_end" | "stop" => handle_session_end(&repo, &args, &payload).await?,
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
        clear_current_session()?;
        println!("{{\"closed_session\":\"{id}\"}}");
    }
    Ok(())
}

async fn handle_user_prompt(
    repo: &SessionsRepository<'_>,
    _args: &HookHandleArgs,
    payload: &serde_json::Value,
) -> anyhow::Result<()> {
    let content =
        first_field(payload, &["user_prompt", "prompt", "content", "message"]).unwrap_or_default();
    record_turn(repo, "user", &content, None, payload).await
}

async fn handle_pre_tool_use(
    repo: &SessionsRepository<'_>,
    _args: &HookHandleArgs,
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
    record_turn(repo, "tool_call", &content, tool_name, payload).await
}

async fn handle_post_tool_use(
    repo: &SessionsRepository<'_>,
    _args: &HookHandleArgs,
    payload: &serde_json::Value,
) -> anyhow::Result<()> {
    let tool_name = payload
        .get("tool_name")
        .and_then(serde_json::Value::as_str)
        .map(String::from);
    let response = first_field(payload, &["tool_response", "result", "output"]).unwrap_or_default();
    record_turn(repo, "tool_result", &response, tool_name, payload).await
}

async fn record_turn(
    repo: &SessionsRepository<'_>,
    role: &str,
    raw_content: &str,
    tool_name: Option<String>,
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
    let redacted_count = captures.len() as i64;
    let final_content = if redacted_count > 0 {
        redact(raw_content)
    } else {
        raw_content.to_string()
    };
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
            payload.get("tool_input").cloned()
        } else {
            Some(serde_json::json!({
                "tool_input": payload.get("tool_input").cloned(),
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
        file_changes: payload.get("file_changes").cloned(),
        redacted_count,
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
}

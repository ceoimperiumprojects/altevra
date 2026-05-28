//! `altevra turn record` — write a single agent interaction turn to SQLite.
//!
//! Designed to be called by hooks (UserPromptSubmit, PostToolUse, ...) and by
//! adapters that have access to per-message metadata. Content is redacted in
//! place via `altevra_secrets::redactor` before persist — Pavle's directive
//! is "full content + tool args" with safety via redaction, not omission.

use altevra_db::{create_pool, run_migrations, SessionsRepository, TurnRow};
use altevra_secrets::{detect_secrets, redact};
use chrono::Utc;
use clap::Args;
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Args)]
pub struct TurnRecordArgs {
    /// Session id (UUID) — from `altevra session start`.
    #[arg(long)]
    pub session_id: String,

    /// Role: user | assistant | system | tool_call | tool_result.
    #[arg(long)]
    pub role: String,

    /// Content (string OR `@-` for stdin OR `@<file>` to read a file).
    #[arg(long)]
    pub content: String,

    /// Tool name (when role=tool_call/tool_result).
    #[arg(long)]
    pub tool_name: Option<String>,

    /// JSON array of tool call descriptors (raw string, will be parsed).
    #[arg(long)]
    pub tool_calls: Option<String>,

    /// Model identifier (e.g. claude-opus-4-7, gpt-5).
    #[arg(long)]
    pub model: Option<String>,

    /// Tokens consumed by this turn (input side).
    #[arg(long)]
    pub tokens_in: Option<i64>,

    /// Tokens produced by this turn (output side).
    #[arg(long)]
    pub tokens_out: Option<i64>,

    /// Latency in milliseconds (assistant turns).
    #[arg(long)]
    pub latency_ms: Option<i64>,

    /// JSON array of {path, before_hash, after_hash} file change descriptors.
    #[arg(long)]
    pub file_changes: Option<String>,

    /// Skip redaction (advanced — typically you want secrets removed).
    #[arg(long)]
    pub no_redact: bool,

    /// SQLite database path.
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,

    /// Emit JSON receipt.
    #[arg(long)]
    pub json: bool,
}

pub async fn run(args: TurnRecordArgs) -> anyhow::Result<()> {
    let session_id = Uuid::parse_str(&args.session_id)
        .map_err(|_| anyhow::anyhow!("Invalid session id: {}", args.session_id))?;

    // Resolve content: literal, stdin, or file
    let raw_content = resolve_content(&args.content)?;

    // Detect + redact secrets unless explicitly disabled
    let (final_content, redacted_count) = if args.no_redact {
        (raw_content, 0i64)
    } else {
        let matches = detect_secrets(&raw_content);
        let count = matches.len() as i64;
        (redact(&raw_content), count)
    };

    let tool_calls = args
        .tool_calls
        .as_deref()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
    let file_changes = args
        .file_changes
        .as_deref()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());

    let pool = create_pool(&args.db.to_string_lossy()).await?;
    run_migrations(&pool).await?;
    let repo = SessionsRepository::new(&pool);
    let turn_idx = repo.next_turn_idx(session_id).await?;

    let turn = TurnRow {
        id: Uuid::new_v4(),
        session_id,
        turn_idx,
        role: args.role.clone(),
        content: final_content,
        tool_calls,
        tool_name: args.tool_name.clone(),
        model: args.model.clone(),
        tokens_in: args.tokens_in,
        tokens_out: args.tokens_out,
        latency_ms: args.latency_ms,
        file_changes,
        redacted_count,
        created_at: Utc::now(),
    };

    repo.record_turn(&turn).await?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "turn_id": turn.id,
                "session_id": session_id,
                "turn_idx": turn_idx,
                "role": args.role,
                "redacted_count": redacted_count,
            }))?
        );
    } else {
        println!(
            "Recorded turn {} (idx {}, {} redacted)",
            turn.id, turn_idx, redacted_count
        );
    }
    Ok(())
}

fn resolve_content(input: &str) -> anyhow::Result<String> {
    if input == "@-" {
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
        Ok(buf)
    } else if let Some(path) = input.strip_prefix('@') {
        Ok(std::fs::read_to_string(path)?)
    } else {
        Ok(input.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use altevra_db::SessionRow;
    use tempfile::TempDir;

    async fn make_session(db: &std::path::Path, tool: &str) -> Uuid {
        let pool = create_pool(&db.to_string_lossy()).await.unwrap();
        run_migrations(&pool).await.unwrap();
        let repo = SessionsRepository::new(&pool);
        let row = SessionRow {
            id: Uuid::new_v4(),
            tool: tool.into(),
            project_id: None,
            project_name: None,
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
        };
        repo.start_session(&row).await.unwrap();
        row.id
    }

    #[tokio::test]
    async fn record_redacts_secrets_by_default() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("altevra.db");
        let session_id = make_session(&db, "claude-code").await;

        run(TurnRecordArgs {
            session_id: session_id.to_string(),
            role: "user".into(),
            content: "My key is sk-ant-1234567890abcdefghijklmnop please use it".into(),
            tool_name: None,
            tool_calls: None,
            model: None,
            tokens_in: Some(15),
            tokens_out: None,
            latency_ms: None,
            file_changes: None,
            no_redact: false,
            db: db.clone(),
            json: true,
        })
        .await
        .unwrap();

        let pool = create_pool(&db.to_string_lossy()).await.unwrap();
        let repo = SessionsRepository::new(&pool);
        let turns = repo.list_turns(session_id, 10).await.unwrap();
        assert_eq!(turns.len(), 1);
        assert!(
            !turns[0]
                .content
                .contains("sk-ant-1234567890abcdefghijklmnop"),
            "secret should be redacted"
        );
        assert!(turns[0].redacted_count > 0);
    }

    #[tokio::test]
    async fn no_redact_preserves_content() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("altevra.db");
        let session_id = make_session(&db, "codex").await;

        run(TurnRecordArgs {
            session_id: session_id.to_string(),
            role: "system".into(),
            content: "no-secret content here".into(),
            tool_name: None,
            tool_calls: None,
            model: None,
            tokens_in: None,
            tokens_out: None,
            latency_ms: None,
            file_changes: None,
            no_redact: true,
            db: db.clone(),
            json: true,
        })
        .await
        .unwrap();

        let pool = create_pool(&db.to_string_lossy()).await.unwrap();
        let repo = SessionsRepository::new(&pool);
        let turns = repo.list_turns(session_id, 10).await.unwrap();
        assert_eq!(turns[0].content, "no-secret content here");
        assert_eq!(turns[0].redacted_count, 0);
    }
}

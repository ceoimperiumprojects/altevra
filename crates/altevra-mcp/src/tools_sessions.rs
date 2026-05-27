//! MCP tools for v0.3.7 Replay & Query: replay_session, search_turns, file_history.

use crate::server::McpResponse;
use serde_json::Value;
use std::path::PathBuf;

const DEFAULT_DB: &str = ".altevra/altevra.db";

fn db_path_from_args(args: &Value) -> PathBuf {
    args.get("db_path")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_DB))
}

async fn open_pool(db_path: &std::path::Path) -> anyhow::Result<sqlx::SqlitePool> {
    let pool = altevra_db::create_pool(&db_path.to_string_lossy()).await?;
    altevra_db::run_migrations(&pool).await?;
    Ok(pool)
}

pub fn handle_replay_session(id: Value, args: &Value) -> McpResponse {
    let session_id = args
        .get("session_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let turn_limit = args
        .get("turn_limit")
        .and_then(|v| v.as_i64())
        .unwrap_or(1000);
    let db_path = db_path_from_args(args);

    if session_id.is_empty() {
        return McpResponse::error(id, -32602, "session_id required");
    }
    let session_uuid = match uuid::Uuid::parse_str(session_id) {
        Ok(u) => u,
        Err(e) => return McpResponse::error(id, -32602, format!("invalid uuid: {e}")),
    };

    let rt = match tokio::runtime::Handle::try_current() {
        Ok(h) => h,
        Err(_) => return McpResponse::error(id, -32603, "no tokio runtime"),
    };

    let result: anyhow::Result<Value> = futures::executor::block_on(async {
        let pool = open_pool(&db_path).await?;
        let repo = altevra_db::SessionsRepository::new(&pool);
        let session = repo
            .get_session(session_uuid)
            .await?
            .ok_or_else(|| anyhow::anyhow!("session not found"))?;
        let turns = repo.list_turns(session_uuid, turn_limit).await?;
        Ok(serde_json::json!({
            "session": {
                "id": session.id,
                "tool": session.tool,
                "project": session.project_name,
                "started_at": session.started_at,
                "ended_at": session.ended_at,
                "turn_count": session.turn_count,
            },
            "turns": turns.iter().map(|t| serde_json::json!({
                "idx": t.turn_idx,
                "role": t.role,
                "content": t.content,
                "tool_name": t.tool_name,
                "created_at": t.created_at,
            })).collect::<Vec<_>>(),
        }))
    });
    let _ = rt; // silence unused

    match result {
        Ok(v) => McpResponse::ok(id, v),
        Err(e) => McpResponse::error(id, -32603, e.to_string()),
    }
}

pub fn handle_search_turns(id: Value, args: &Value) -> McpResponse {
    let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
    let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(10);
    let project = args.get("project").and_then(|v| v.as_str());
    let tool = args.get("tool").and_then(|v| v.as_str());
    let db_path = db_path_from_args(args);

    if query.is_empty() {
        return McpResponse::error(id, -32602, "query required");
    }

    let result: anyhow::Result<Value> = futures::executor::block_on(async {
        let pool = open_pool(&db_path).await?;
        let repo = altevra_db::SessionsRepository::new(&pool);
        let hits = repo.search_turns(query, project, tool, limit).await?;
        Ok(serde_json::json!({
            "query": query,
            "count": hits.len(),
            "results": hits.iter().map(|(t, score)| serde_json::json!({
                "session_id": t.session_id,
                "turn_idx": t.turn_idx,
                "role": t.role,
                "score": score,
                "snippet": t.content.chars().take(220).collect::<String>(),
                "created_at": t.created_at,
            })).collect::<Vec<_>>(),
        }))
    });

    match result {
        Ok(v) => McpResponse::ok(id, v),
        Err(e) => McpResponse::error(id, -32603, e.to_string()),
    }
}

pub fn handle_file_history(id: Value, args: &Value) -> McpResponse {
    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
    let db_path = db_path_from_args(args);

    if path.is_empty() {
        return McpResponse::error(id, -32602, "path required");
    }

    let result: anyhow::Result<Value> = futures::executor::block_on(async {
        let pool = open_pool(&db_path).await?;
        let repo = altevra_db::SessionsRepository::new(&pool);
        let history = repo.file_history(path, limit).await?;
        Ok(serde_json::json!({
            "path": path,
            "count": history.len(),
            "history": history.iter().map(|c| serde_json::json!({
                "id": c.id,
                "session_id": c.session_id,
                "turn_id": c.turn_id,
                "before_hash": c.before_hash,
                "after_hash": c.after_hash,
                "diff_summary": c.diff_summary,
                "actor_id": c.actor_id,
                "created_at": c.created_at,
            })).collect::<Vec<_>>(),
        }))
    });

    match result {
        Ok(v) => McpResponse::ok(id, v),
        Err(e) => McpResponse::error(id, -32603, e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use altevra_db::{FileChangeRow, SessionRow, SessionsRepository, TurnRow};
    use chrono::Utc;
    use tempfile::TempDir;
    use uuid::Uuid;

    async fn seed(db: &std::path::Path) -> Uuid {
        let pool = altevra_db::create_pool(&db.to_string_lossy())
            .await
            .unwrap();
        altevra_db::run_migrations(&pool).await.unwrap();
        let repo = SessionsRepository::new(&pool);
        let s = SessionRow {
            id: Uuid::new_v4(),
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
        };
        repo.start_session(&s).await.unwrap();
        repo.record_turn(&TurnRow {
            id: Uuid::new_v4(),
            session_id: s.id,
            turn_idx: 0,
            role: "user".into(),
            content: "Talk about GTM strategy and Rust patterns".into(),
            tool_calls: None,
            tool_name: None,
            model: None,
            tokens_in: None,
            tokens_out: None,
            latency_ms: None,
            file_changes: None,
            redacted_count: 0,
            created_at: Utc::now(),
        })
        .await
        .unwrap();
        repo.record_file_change(&FileChangeRow {
            id: Uuid::new_v4(),
            session_id: Some(s.id),
            turn_id: None,
            path: "src/main.rs".into(),
            before_hash: Some("a".into()),
            after_hash: Some("b".into()),
            diff_summary: Some("edit".into()),
            actor_type: "agent".into(),
            actor_id: Some("claude".into()),
            created_at: Utc::now(),
        })
        .await
        .unwrap();
        s.id
    }

    #[tokio::test]
    async fn replay_session_returns_turns() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("a.db");
        let sid = seed(&db).await;
        let args = serde_json::json!({
            "session_id": sid.to_string(),
            "db_path": db.to_string_lossy(),
        });
        let resp = handle_replay_session(serde_json::json!(1), &args);
        assert!(resp.error.is_none(), "{:?}", resp.error);
        let result = resp.result.unwrap();
        assert_eq!(result["session"]["id"], sid.to_string());
        assert!(result["turns"].as_array().unwrap().len() >= 1);
    }

    #[tokio::test]
    async fn search_turns_returns_match() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("a.db");
        seed(&db).await;
        let args = serde_json::json!({
            "query": "GTM",
            "db_path": db.to_string_lossy(),
        });
        let resp = handle_search_turns(serde_json::json!(1), &args);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert!(result["count"].as_i64().unwrap() >= 1);
    }

    #[tokio::test]
    async fn file_history_returns_changes() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("a.db");
        seed(&db).await;
        let args = serde_json::json!({
            "path": "src/main.rs",
            "db_path": db.to_string_lossy(),
        });
        let resp = handle_file_history(serde_json::json!(1), &args);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["count"].as_i64().unwrap(), 1);
    }
}

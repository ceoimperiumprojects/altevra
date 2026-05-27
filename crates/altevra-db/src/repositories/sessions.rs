//! Sessions + Turns + FileChanges repositories — v0.3.1 omniscient logging.
//!
//! Records every agent interaction at turn-level granularity. Content is
//! always written as-stored (caller is responsible for redaction). Designed
//! for high-write throughput: each `record_turn` is a single INSERT, no joins.

use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::util::{opt_ts_from_text, opt_uuid_from_text, ts_from_text, ts_to_text, uuid_from_text};

#[derive(Debug, Clone)]
pub struct SessionRow {
    pub id: Uuid,
    pub tool: String,
    pub project_id: Option<Uuid>,
    pub project_name: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub summary: Option<String>,
    pub tokens_in_total: i64,
    pub tokens_out_total: i64,
    pub cost_usd_estimate: f64,
    pub turn_count: i64,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct TurnRow {
    pub id: Uuid,
    pub session_id: Uuid,
    pub turn_idx: i64,
    pub role: String,
    pub content: String,
    pub tool_calls: Option<serde_json::Value>,
    pub tool_name: Option<String>,
    pub model: Option<String>,
    pub tokens_in: Option<i64>,
    pub tokens_out: Option<i64>,
    pub latency_ms: Option<i64>,
    pub file_changes: Option<serde_json::Value>,
    pub redacted_count: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct FileChangeRow {
    pub id: Uuid,
    pub session_id: Option<Uuid>,
    pub turn_id: Option<Uuid>,
    pub path: String,
    pub before_hash: Option<String>,
    pub after_hash: Option<String>,
    pub diff_summary: Option<String>,
    pub actor_type: String,
    pub actor_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub struct SessionsRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> SessionsRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn start_session(&self, s: &SessionRow) -> anyhow::Result<()> {
        sqlx::query(
            r#"INSERT INTO sessions
                (id, tool, project_id, project_name, started_at, metadata)
               VALUES (?, ?, ?, ?, ?, ?)"#,
        )
        .bind(s.id.to_string())
        .bind(&s.tool)
        .bind(s.project_id.map(|u| u.to_string()))
        .bind(s.project_name.as_deref())
        .bind(ts_to_text(&s.started_at))
        .bind(s.metadata.to_string())
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn end_session(&self, session_id: Uuid, summary: Option<&str>) -> anyhow::Result<()> {
        let now = ts_to_text(&Utc::now());
        sqlx::query(
            r#"UPDATE sessions
               SET ended_at = ?, summary = COALESCE(?, summary)
               WHERE id = ?"#,
        )
        .bind(now)
        .bind(summary)
        .bind(session_id.to_string())
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn record_turn(&self, t: &TurnRow) -> anyhow::Result<()> {
        sqlx::query(
            r#"INSERT INTO turns
                (id, session_id, turn_idx, role, content, tool_calls, tool_name,
                 model, tokens_in, tokens_out, latency_ms, file_changes,
                 redacted_count, created_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(t.id.to_string())
        .bind(t.session_id.to_string())
        .bind(t.turn_idx)
        .bind(&t.role)
        .bind(&t.content)
        .bind(t.tool_calls.as_ref().map(|v| v.to_string()))
        .bind(t.tool_name.as_deref())
        .bind(t.model.as_deref())
        .bind(t.tokens_in)
        .bind(t.tokens_out)
        .bind(t.latency_ms)
        .bind(t.file_changes.as_ref().map(|v| v.to_string()))
        .bind(t.redacted_count)
        .bind(ts_to_text(&t.created_at))
        .execute(self.pool)
        .await?;

        // Update session aggregates.
        sqlx::query(
            r#"UPDATE sessions
               SET turn_count = turn_count + 1,
                   tokens_in_total = tokens_in_total + COALESCE(?, 0),
                   tokens_out_total = tokens_out_total + COALESCE(?, 0)
               WHERE id = ?"#,
        )
        .bind(t.tokens_in)
        .bind(t.tokens_out)
        .bind(t.session_id.to_string())
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn record_file_change(&self, c: &FileChangeRow) -> anyhow::Result<()> {
        sqlx::query(
            r#"INSERT INTO file_changes
                (id, session_id, turn_id, path, before_hash, after_hash,
                 diff_summary, actor_type, actor_id, created_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(c.id.to_string())
        .bind(c.session_id.map(|u| u.to_string()))
        .bind(c.turn_id.map(|u| u.to_string()))
        .bind(&c.path)
        .bind(c.before_hash.as_deref())
        .bind(c.after_hash.as_deref())
        .bind(c.diff_summary.as_deref())
        .bind(&c.actor_type)
        .bind(c.actor_id.as_deref())
        .bind(ts_to_text(&c.created_at))
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn next_turn_idx(&self, session_id: Uuid) -> anyhow::Result<i64> {
        let row = sqlx::query("SELECT turn_count FROM sessions WHERE id = ?")
            .bind(session_id.to_string())
            .fetch_optional(self.pool)
            .await?;
        Ok(row.map(|r| r.get::<i64, _>("turn_count")).unwrap_or(0))
    }

    pub async fn list_sessions(
        &self,
        tool: Option<&str>,
        project: Option<&str>,
        limit: i64,
    ) -> anyhow::Result<Vec<SessionRow>> {
        let rows = match (tool, project) {
            (Some(t), Some(p)) => {
                sqlx::query(
                    r#"SELECT * FROM sessions WHERE tool = ? AND project_name = ?
                   ORDER BY started_at DESC LIMIT ?"#,
                )
                .bind(t)
                .bind(p)
                .bind(limit)
                .fetch_all(self.pool)
                .await?
            }
            (Some(t), None) => {
                sqlx::query(
                    r#"SELECT * FROM sessions WHERE tool = ?
                   ORDER BY started_at DESC LIMIT ?"#,
                )
                .bind(t)
                .bind(limit)
                .fetch_all(self.pool)
                .await?
            }
            (None, Some(p)) => {
                sqlx::query(
                    r#"SELECT * FROM sessions WHERE project_name = ?
                   ORDER BY started_at DESC LIMIT ?"#,
                )
                .bind(p)
                .bind(limit)
                .fetch_all(self.pool)
                .await?
            }
            (None, None) => {
                sqlx::query(r#"SELECT * FROM sessions ORDER BY started_at DESC LIMIT ?"#)
                    .bind(limit)
                    .fetch_all(self.pool)
                    .await?
            }
        };

        Ok(rows
            .into_iter()
            .map(|r| SessionRow {
                id: uuid_from_text(r.get::<String, _>("id")),
                tool: r.get("tool"),
                project_id: opt_uuid_from_text(r.get::<Option<String>, _>("project_id")),
                project_name: r.get("project_name"),
                started_at: ts_from_text(r.get::<String, _>("started_at")),
                ended_at: opt_ts_from_text(r.get::<Option<String>, _>("ended_at")),
                summary: r.get("summary"),
                tokens_in_total: r.get("tokens_in_total"),
                tokens_out_total: r.get("tokens_out_total"),
                cost_usd_estimate: r.get("cost_usd_estimate"),
                turn_count: r.get("turn_count"),
                metadata: serde_json::from_str(&r.get::<String, _>("metadata"))
                    .unwrap_or(serde_json::json!({})),
            })
            .collect())
    }

    pub async fn get_session(&self, session_id: Uuid) -> anyhow::Result<Option<SessionRow>> {
        let row = sqlx::query("SELECT * FROM sessions WHERE id = ?")
            .bind(session_id.to_string())
            .fetch_optional(self.pool)
            .await?;
        Ok(row.map(|r| SessionRow {
            id: uuid_from_text(r.get::<String, _>("id")),
            tool: r.get("tool"),
            project_id: opt_uuid_from_text(r.get::<Option<String>, _>("project_id")),
            project_name: r.get("project_name"),
            started_at: ts_from_text(r.get::<String, _>("started_at")),
            ended_at: opt_ts_from_text(r.get::<Option<String>, _>("ended_at")),
            summary: r.get("summary"),
            tokens_in_total: r.get("tokens_in_total"),
            tokens_out_total: r.get("tokens_out_total"),
            cost_usd_estimate: r.get("cost_usd_estimate"),
            turn_count: r.get("turn_count"),
            metadata: serde_json::from_str(&r.get::<String, _>("metadata"))
                .unwrap_or(serde_json::json!({})),
        }))
    }

    pub async fn list_turns(&self, session_id: Uuid, limit: i64) -> anyhow::Result<Vec<TurnRow>> {
        let rows = sqlx::query(
            r#"SELECT * FROM turns WHERE session_id = ?
               ORDER BY turn_idx ASC LIMIT ?"#,
        )
        .bind(session_id.to_string())
        .bind(limit)
        .fetch_all(self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| TurnRow {
                id: uuid_from_text(r.get::<String, _>("id")),
                session_id: uuid_from_text(r.get::<String, _>("session_id")),
                turn_idx: r.get("turn_idx"),
                role: r.get("role"),
                content: r.get("content"),
                tool_calls: r
                    .get::<Option<String>, _>("tool_calls")
                    .and_then(|s| serde_json::from_str(&s).ok()),
                tool_name: r.get("tool_name"),
                model: r.get("model"),
                tokens_in: r.get("tokens_in"),
                tokens_out: r.get("tokens_out"),
                latency_ms: r.get("latency_ms"),
                file_changes: r
                    .get::<Option<String>, _>("file_changes")
                    .and_then(|s| serde_json::from_str(&s).ok()),
                redacted_count: r.get("redacted_count"),
                created_at: ts_from_text(r.get::<String, _>("created_at")),
            })
            .collect())
    }

    pub async fn file_history(&self, path: &str, limit: i64) -> anyhow::Result<Vec<FileChangeRow>> {
        let rows = sqlx::query(
            r#"SELECT * FROM file_changes WHERE path = ?
               ORDER BY created_at DESC LIMIT ?"#,
        )
        .bind(path)
        .bind(limit)
        .fetch_all(self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| FileChangeRow {
                id: uuid_from_text(r.get::<String, _>("id")),
                session_id: opt_uuid_from_text(r.get::<Option<String>, _>("session_id")),
                turn_id: opt_uuid_from_text(r.get::<Option<String>, _>("turn_id")),
                path: r.get("path"),
                before_hash: r.get("before_hash"),
                after_hash: r.get("after_hash"),
                diff_summary: r.get("diff_summary"),
                actor_type: r.get("actor_type"),
                actor_id: r.get("actor_id"),
                created_at: ts_from_text(r.get::<String, _>("created_at")),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn setup() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::pool::run_migrations(&pool).await.unwrap();
        pool
    }

    fn sample_session() -> SessionRow {
        SessionRow {
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
        }
    }

    #[tokio::test]
    async fn start_and_end_session_roundtrip() {
        let pool = setup().await;
        let repo = SessionsRepository::new(&pool);
        let s = sample_session();
        repo.start_session(&s).await.unwrap();

        let fetched = repo.get_session(s.id).await.unwrap().unwrap();
        assert_eq!(fetched.tool, "claude-code");
        assert!(fetched.ended_at.is_none());

        repo.end_session(s.id, Some("wrapped up")).await.unwrap();
        let fetched = repo.get_session(s.id).await.unwrap().unwrap();
        assert!(fetched.ended_at.is_some());
        assert_eq!(fetched.summary.as_deref(), Some("wrapped up"));
    }

    #[tokio::test]
    async fn record_turn_increments_session_counters() {
        let pool = setup().await;
        let repo = SessionsRepository::new(&pool);
        let s = sample_session();
        repo.start_session(&s).await.unwrap();

        let turn = TurnRow {
            id: Uuid::new_v4(),
            session_id: s.id,
            turn_idx: 0,
            role: "user".into(),
            content: "hello".into(),
            tool_calls: None,
            tool_name: None,
            model: Some("claude-opus-4-7".into()),
            tokens_in: Some(10),
            tokens_out: None,
            latency_ms: None,
            file_changes: None,
            redacted_count: 0,
            created_at: Utc::now(),
        };
        repo.record_turn(&turn).await.unwrap();

        let fetched = repo.get_session(s.id).await.unwrap().unwrap();
        assert_eq!(fetched.turn_count, 1);
        assert_eq!(fetched.tokens_in_total, 10);
    }

    #[tokio::test]
    async fn list_turns_returns_in_order() {
        let pool = setup().await;
        let repo = SessionsRepository::new(&pool);
        let s = sample_session();
        repo.start_session(&s).await.unwrap();

        for i in 0..3 {
            let turn = TurnRow {
                id: Uuid::new_v4(),
                session_id: s.id,
                turn_idx: i,
                role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
                content: format!("turn {i}"),
                tool_calls: None,
                tool_name: None,
                model: None,
                tokens_in: None,
                tokens_out: None,
                latency_ms: None,
                file_changes: None,
                redacted_count: 0,
                created_at: Utc::now(),
            };
            repo.record_turn(&turn).await.unwrap();
        }
        let turns = repo.list_turns(s.id, 10).await.unwrap();
        assert_eq!(turns.len(), 3);
        assert_eq!(turns[0].content, "turn 0");
        assert_eq!(turns[2].content, "turn 2");
    }

    #[tokio::test]
    async fn record_file_change_and_history() {
        let pool = setup().await;
        let repo = SessionsRepository::new(&pool);
        let change = FileChangeRow {
            id: Uuid::new_v4(),
            session_id: None,
            turn_id: None,
            path: "src/main.rs".into(),
            before_hash: Some("abc123".into()),
            after_hash: Some("def456".into()),
            diff_summary: Some("+5 -2".into()),
            actor_type: "agent".into(),
            actor_id: Some("claude-opus-4-7".into()),
            created_at: Utc::now(),
        };
        repo.record_file_change(&change).await.unwrap();
        let history = repo.file_history("src/main.rs", 10).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].diff_summary.as_deref(), Some("+5 -2"));
    }

    #[tokio::test]
    async fn list_sessions_filters_by_tool() {
        let pool = setup().await;
        let repo = SessionsRepository::new(&pool);
        let mut a = sample_session();
        a.tool = "claude-code".into();
        let mut b = sample_session();
        b.tool = "codex".into();
        repo.start_session(&a).await.unwrap();
        repo.start_session(&b).await.unwrap();

        let list = repo
            .list_sessions(Some("claude-code"), None, 10)
            .await
            .unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].tool, "claude-code");
    }
}

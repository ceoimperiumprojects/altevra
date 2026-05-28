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
    /// Tool-native session id (e.g. Claude Code JSONL UUID, Codex thread id,
    /// Cursor sessionId). Set only by Analyze Everything imports — live
    /// `altevra session start` leaves it None. Combined with `tool` it is the
    /// idempotency key for re-runs.
    pub external_id: Option<String>,
    /// Absolute path on disk we imported from (for debugging / incremental).
    pub imported_from: Option<String>,
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
                (id, tool, project_id, project_name, started_at, metadata,
                 external_id, imported_from)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(s.id.to_string())
        .bind(&s.tool)
        .bind(s.project_id.map(|u| u.to_string()))
        .bind(s.project_name.as_deref())
        .bind(ts_to_text(&s.started_at))
        .bind(s.metadata.to_string())
        .bind(s.external_id.as_deref())
        .bind(s.imported_from.as_deref())
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Look up an existing session by `(tool, external_id)`. Returns the row
    /// when found, used by Analyze Everything to detect duplicates.
    pub async fn find_by_external(
        &self,
        tool: &str,
        external_id: &str,
    ) -> anyhow::Result<Option<SessionRow>> {
        let row = sqlx::query("SELECT * FROM sessions WHERE tool = ? AND external_id = ?")
            .bind(tool)
            .bind(external_id)
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
            external_id: r.get("external_id"),
            imported_from: r.get("imported_from"),
        }))
    }

    /// Idempotent insert: if a session with `(tool, external_id)` already
    /// exists, returns `Ok(None)` and leaves it untouched. Otherwise inserts
    /// the row and returns `Ok(Some(id))`. Caller decides whether to import
    /// turns based on the result.
    pub async fn upsert_imported(&self, s: &SessionRow) -> anyhow::Result<Option<Uuid>> {
        let ext = match s.external_id.as_deref() {
            Some(e) if !e.is_empty() => e,
            _ => anyhow::bail!("upsert_imported requires external_id on SessionRow"),
        };
        if let Some(existing) = self.find_by_external(&s.tool, ext).await? {
            tracing::debug!(
                tool = %s.tool,
                external_id = ext,
                existing_id = %existing.id,
                "skip duplicate"
            );
            return Ok(None);
        }
        self.start_session(s).await?;
        Ok(Some(s.id))
    }

    /// Set or replace the AI-generated session summary. Used after LLM
    /// summarization of imported sessions.
    pub async fn set_summary(&self, session_id: Uuid, summary: &str) -> anyhow::Result<()> {
        sqlx::query("UPDATE sessions SET summary = ? WHERE id = ?")
            .bind(summary)
            .bind(session_id.to_string())
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
                external_id: r.get("external_id"),
                imported_from: r.get("imported_from"),
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
            external_id: r.get("external_id"),
            imported_from: r.get("imported_from"),
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

    /// List sessions with an optional `since` cutoff. Empty `since` = no filter.
    pub async fn list_sessions_since(
        &self,
        tool: Option<&str>,
        project: Option<&str>,
        since: Option<DateTime<Utc>>,
        limit: i64,
    ) -> anyhow::Result<Vec<SessionRow>> {
        let since_str = since.map(|t| ts_to_text(&t));
        let rows = match (tool, project, &since_str) {
            (Some(t), Some(p), Some(s)) => {
                sqlx::query(
                    r#"SELECT * FROM sessions
                   WHERE tool = ? AND project_name = ? AND started_at >= ?
                   ORDER BY started_at DESC LIMIT ?"#,
                )
                .bind(t)
                .bind(p)
                .bind(s)
                .bind(limit)
                .fetch_all(self.pool)
                .await?
            }
            (Some(t), None, Some(s)) => {
                sqlx::query(
                    r#"SELECT * FROM sessions WHERE tool = ? AND started_at >= ?
                   ORDER BY started_at DESC LIMIT ?"#,
                )
                .bind(t)
                .bind(s)
                .bind(limit)
                .fetch_all(self.pool)
                .await?
            }
            (None, Some(p), Some(s)) => {
                sqlx::query(
                    r#"SELECT * FROM sessions WHERE project_name = ? AND started_at >= ?
                   ORDER BY started_at DESC LIMIT ?"#,
                )
                .bind(p)
                .bind(s)
                .bind(limit)
                .fetch_all(self.pool)
                .await?
            }
            (None, None, Some(s)) => {
                sqlx::query(
                    r#"SELECT * FROM sessions WHERE started_at >= ?
                   ORDER BY started_at DESC LIMIT ?"#,
                )
                .bind(s)
                .bind(limit)
                .fetch_all(self.pool)
                .await?
            }
            _ => {
                return self.list_sessions(tool, project, limit).await;
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
                external_id: r.get("external_id"),
                imported_from: r.get("imported_from"),
            })
            .collect())
    }

    /// Full-text search across turn content. Uses BM25-style ranking via simple
    /// token overlap (no SQLite FTS5 dependency — works in-process).
    /// Returns the top `limit` turns by score; ties broken by recency.
    pub async fn search_turns(
        &self,
        query: &str,
        project: Option<&str>,
        tool: Option<&str>,
        limit: i64,
    ) -> anyhow::Result<Vec<(TurnRow, f32)>> {
        let q_tokens: Vec<String> = query
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| t.len() > 2)
            .map(String::from)
            .collect();
        if q_tokens.is_empty() {
            return Ok(vec![]);
        }

        // Pull candidate turns via SQL LIKE per token (OR), join to sessions
        // when project/tool filter present.
        let mut sql = String::from(
            "SELECT t.*, s.tool AS s_tool, s.project_name AS s_project
             FROM turns t LEFT JOIN sessions s ON t.session_id = s.id WHERE (",
        );
        for (i, _) in q_tokens.iter().enumerate() {
            if i > 0 {
                sql.push_str(" OR ");
            }
            sql.push_str("LOWER(t.content) LIKE ?");
        }
        sql.push(')');
        if tool.is_some() {
            sql.push_str(" AND s.tool = ?");
        }
        if project.is_some() {
            sql.push_str(" AND s.project_name = ?");
        }
        sql.push_str(" ORDER BY t.created_at DESC LIMIT ?");

        let mut q = sqlx::query(&sql);
        for t in &q_tokens {
            q = q.bind(format!("%{t}%"));
        }
        if let Some(t) = tool {
            q = q.bind(t);
        }
        if let Some(p) = project {
            q = q.bind(p);
        }
        // Cap candidate set generously; final ranking is in-process.
        q = q.bind(limit * 4);

        let rows = q.fetch_all(self.pool).await?;

        let mut scored: Vec<(TurnRow, f32)> = rows
            .into_iter()
            .map(|r| {
                let content: String = r.get("content");
                let lc = content.to_lowercase();
                let mut score = 0.0_f32;
                for t in &q_tokens {
                    let count = lc.matches(t.as_str()).count();
                    if count > 0 {
                        score += (count as f32).ln_1p();
                    }
                }
                let row = TurnRow {
                    id: uuid_from_text(r.get::<String, _>("id")),
                    session_id: uuid_from_text(r.get::<String, _>("session_id")),
                    turn_idx: r.get("turn_idx"),
                    role: r.get("role"),
                    content,
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
                };
                (row, score)
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit as usize);
        Ok(scored)
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
            external_id: None,
            imported_from: None,
        }
    }

    #[tokio::test]
    async fn upsert_imported_is_idempotent() {
        let pool = setup().await;
        let repo = SessionsRepository::new(&pool);
        let mut s = sample_session();
        s.external_id = Some("uuid-abc-123".into());
        s.imported_from = Some("/tmp/fixture.jsonl".into());

        let first = repo.upsert_imported(&s).await.unwrap();
        assert_eq!(first, Some(s.id));

        // Second call with the same external_id must skip.
        let mut s2 = sample_session();
        s2.external_id = Some("uuid-abc-123".into());
        let second = repo.upsert_imported(&s2).await.unwrap();
        assert!(second.is_none());

        // find_by_external returns the original row.
        let found = repo
            .find_by_external("claude-code", "uuid-abc-123")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.id, s.id);
    }

    #[tokio::test]
    async fn upsert_requires_external_id() {
        let pool = setup().await;
        let repo = SessionsRepository::new(&pool);
        let s = sample_session();
        let result = repo.upsert_imported(&s).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn set_summary_updates_row() {
        let pool = setup().await;
        let repo = SessionsRepository::new(&pool);
        let s = sample_session();
        repo.start_session(&s).await.unwrap();
        repo.set_summary(s.id, "Built v0.3.8 importer.")
            .await
            .unwrap();
        let fetched = repo.get_session(s.id).await.unwrap().unwrap();
        assert_eq!(fetched.summary.as_deref(), Some("Built v0.3.8 importer."));
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

    #[tokio::test]
    async fn list_sessions_since_filters_by_time() {
        let pool = setup().await;
        let repo = SessionsRepository::new(&pool);
        // Old session
        let mut old = sample_session();
        old.started_at = Utc::now() - chrono::Duration::days(30);
        repo.start_session(&old).await.unwrap();
        // Recent session
        let recent = sample_session();
        repo.start_session(&recent).await.unwrap();

        let cutoff = Utc::now() - chrono::Duration::days(7);
        let list = repo
            .list_sessions_since(None, None, Some(cutoff), 10)
            .await
            .unwrap();
        // Only the recent one should match
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, recent.id);
    }

    #[tokio::test]
    async fn search_turns_finds_keyword_matches() {
        let pool = setup().await;
        let repo = SessionsRepository::new(&pool);
        let s = sample_session();
        repo.start_session(&s).await.unwrap();
        for (i, content) in [
            "unrelated text",
            "rust agent framework discussion",
            "more rust",
        ]
        .iter()
        .enumerate()
        {
            let t = TurnRow {
                id: Uuid::new_v4(),
                session_id: s.id,
                turn_idx: i as i64,
                role: "user".into(),
                content: (*content).into(),
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
            repo.record_turn(&t).await.unwrap();
        }

        let hits = repo.search_turns("rust", None, None, 10).await.unwrap();
        assert_eq!(hits.len(), 2);
        // Both matching turns should have positive scores
        for (_, score) in &hits {
            assert!(*score > 0.0);
        }
    }

    #[tokio::test]
    async fn search_turns_filters_by_project() {
        let pool = setup().await;
        let repo = SessionsRepository::new(&pool);
        let mut a = sample_session();
        a.project_name = Some("altevra".into());
        let mut b = sample_session();
        b.project_name = Some("revesta".into());
        repo.start_session(&a).await.unwrap();
        repo.start_session(&b).await.unwrap();

        for (sid, content) in [(a.id, "rust altevra core"), (b.id, "rust food surplus")] {
            let t = TurnRow {
                id: Uuid::new_v4(),
                session_id: sid,
                turn_idx: 0,
                role: "user".into(),
                content: content.into(),
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
            repo.record_turn(&t).await.unwrap();
        }

        let altevra_hits = repo
            .search_turns("rust", Some("altevra"), None, 10)
            .await
            .unwrap();
        assert_eq!(altevra_hits.len(), 1);
        assert!(altevra_hits[0].0.content.contains("altevra core"));
    }

    #[tokio::test]
    async fn search_turns_empty_query_returns_empty() {
        let pool = setup().await;
        let repo = SessionsRepository::new(&pool);
        let s = sample_session();
        repo.start_session(&s).await.unwrap();
        let hits = repo.search_turns("", None, None, 10).await.unwrap();
        assert!(hits.is_empty());
    }
}

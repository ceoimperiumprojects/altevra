use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::util::{opt_uuid_from_text, ts_from_text, ts_to_text, uuid_from_text};

#[derive(Debug, Clone)]
pub struct HookRow {
    pub id: Uuid,
    pub slug: String,
    pub version: String,
    pub source_file: String,
    pub checksum: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct HookRunRow {
    pub id: Uuid,
    pub hook_slug: String,
    pub tool_name: String,
    pub project_id: Option<Uuid>,
    pub payload: serde_json::Value,
    pub result: serde_json::Value,
    pub success: bool,
    pub error_message: Option<String>,
    pub duration_ms: i64,
    pub created_at: DateTime<Utc>,
}

pub struct HooksRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> HooksRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn upsert_hook(&self, row: &HookRow) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO hooks (id, slug, version, source_file, checksum, status, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT (slug) DO UPDATE SET
                version = excluded.version,
                source_file = excluded.source_file,
                checksum = excluded.checksum,
                status = excluded.status,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(row.id.to_string())
        .bind(&row.slug)
        .bind(&row.version)
        .bind(&row.source_file)
        .bind(&row.checksum)
        .bind(&row.status)
        .bind(ts_to_text(&row.created_at))
        .bind(ts_to_text(&row.updated_at))
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_hooks(&self) -> anyhow::Result<Vec<HookRow>> {
        let rows = sqlx::query(
            r#"SELECT id, slug, version, source_file, checksum, status, created_at, updated_at
               FROM hooks ORDER BY slug"#,
        )
        .fetch_all(self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| HookRow {
                id: uuid_from_text(r.get::<String, _>("id")),
                slug: r.get("slug"),
                version: r.get("version"),
                source_file: r.get("source_file"),
                checksum: r.get("checksum"),
                status: r.get("status"),
                created_at: ts_from_text(r.get::<String, _>("created_at")),
                updated_at: ts_from_text(r.get::<String, _>("updated_at")),
            })
            .collect())
    }

    pub async fn log_run(&self, run: &HookRunRow) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO hook_runs (id, hook_slug, tool_name, project_id, payload, result,
                success, error_message, duration_ms, created_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(run.id.to_string())
        .bind(&run.hook_slug)
        .bind(&run.tool_name)
        .bind(run.project_id.map(|u| u.to_string()))
        .bind(run.payload.to_string())
        .bind(run.result.to_string())
        .bind(if run.success { 1_i64 } else { 0_i64 })
        .bind(run.error_message.as_deref())
        .bind(run.duration_ms)
        .bind(ts_to_text(&run.created_at))
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_recent_runs(
        &self,
        hook_slug: &str,
        limit: i64,
    ) -> anyhow::Result<Vec<HookRunRow>> {
        let rows = sqlx::query(
            r#"SELECT id, hook_slug, tool_name, project_id, payload, result, success,
               error_message, duration_ms, created_at
               FROM hook_runs WHERE hook_slug = ?
               ORDER BY created_at DESC LIMIT ?"#,
        )
        .bind(hook_slug)
        .bind(limit)
        .fetch_all(self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| HookRunRow {
                id: uuid_from_text(r.get::<String, _>("id")),
                hook_slug: r.get("hook_slug"),
                tool_name: r.get("tool_name"),
                project_id: opt_uuid_from_text(r.get::<Option<String>, _>("project_id")),
                payload: r
                    .get::<Option<String>, _>("payload")
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_else(|| serde_json::Value::Object(Default::default())),
                result: r
                    .get::<Option<String>, _>("result")
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_else(|| serde_json::Value::Object(Default::default())),
                success: r.get::<i64, _>("success") != 0,
                error_message: r.get("error_message"),
                duration_ms: r.get("duration_ms"),
                created_at: ts_from_text(r.get::<String, _>("created_at")),
            })
            .collect())
    }
}

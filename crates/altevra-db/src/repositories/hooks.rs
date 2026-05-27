use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

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
    pool: &'a PgPool,
}

impl<'a> HooksRepository<'a> {
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }

    pub async fn upsert_hook(&self, row: &HookRow) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO hooks (id, slug, version, source_file, checksum, status, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (slug) DO UPDATE SET
                version = EXCLUDED.version,
                source_file = EXCLUDED.source_file,
                checksum = EXCLUDED.checksum,
                status = EXCLUDED.status,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(row.id)
        .bind(&row.slug)
        .bind(&row.version)
        .bind(&row.source_file)
        .bind(&row.checksum)
        .bind(&row.status)
        .bind(row.created_at)
        .bind(row.updated_at)
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
                id: r.get("id"),
                slug: r.get("slug"),
                version: r.get("version"),
                source_file: r.get("source_file"),
                checksum: r.get("checksum"),
                status: r.get("status"),
                created_at: r.get("created_at"),
                updated_at: r.get("updated_at"),
            })
            .collect())
    }

    pub async fn log_run(&self, run: &HookRunRow) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO hook_runs (id, hook_slug, tool_name, project_id, payload, result,
                success, error_message, duration_ms, created_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            "#,
        )
        .bind(run.id)
        .bind(&run.hook_slug)
        .bind(&run.tool_name)
        .bind(run.project_id)
        .bind(&run.payload)
        .bind(&run.result)
        .bind(run.success)
        .bind(run.error_message.as_deref())
        .bind(run.duration_ms)
        .bind(run.created_at)
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
               FROM hook_runs WHERE hook_slug = $1
               ORDER BY created_at DESC LIMIT $2"#,
        )
        .bind(hook_slug)
        .bind(limit)
        .fetch_all(self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| HookRunRow {
                id: r.get("id"),
                hook_slug: r.get("hook_slug"),
                tool_name: r.get("tool_name"),
                project_id: r.get("project_id"),
                payload: r.get("payload"),
                result: r.get("result"),
                success: r.get("success"),
                error_message: r.get("error_message"),
                duration_ms: r.get("duration_ms"),
                created_at: r.get("created_at"),
            })
            .collect())
    }
}

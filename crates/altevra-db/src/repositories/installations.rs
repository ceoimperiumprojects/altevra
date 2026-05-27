use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

pub struct ToolInstallationRow {
    pub id: Uuid,
    pub tool_name: String,
    pub project_id: Option<Uuid>,
    pub adapter_version: String,
    pub installed_at: DateTime<Utc>,
    pub last_verified_at: Option<DateTime<Utc>>,
    pub status: String,
    pub metadata: serde_json::Value,
}

pub struct InstalledComponentRow {
    pub id: Uuid,
    pub installation_id: Uuid,
    pub component_type: String,
    pub component_slug: String,
    pub installed_version: String,
    pub installed_path: String,
    pub checksum: String,
    pub status: String,
    pub last_checked_at: Option<DateTime<Utc>>,
}

pub struct InstallationsRepository<'a> {
    pool: &'a PgPool,
}

impl<'a> InstallationsRepository<'a> {
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }

    pub async fn upsert_installation(&self, row: &ToolInstallationRow) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO tool_installations (id, tool_name, project_id, adapter_version,
                installed_at, last_verified_at, status, metadata)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (tool_name, project_id) DO UPDATE SET
                adapter_version = EXCLUDED.adapter_version,
                last_verified_at = EXCLUDED.last_verified_at,
                status = EXCLUDED.status,
                metadata = EXCLUDED.metadata
            "#,
        )
        .bind(row.id)
        .bind(&row.tool_name)
        .bind(row.project_id)
        .bind(&row.adapter_version)
        .bind(row.installed_at)
        .bind(row.last_verified_at)
        .bind(&row.status)
        .bind(&row.metadata)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn find_installation(
        &self,
        tool_name: &str,
        project_id: Option<Uuid>,
    ) -> anyhow::Result<Option<ToolInstallationRow>> {
        let row = if let Some(pid) = project_id {
            sqlx::query(
                r#"SELECT id, tool_name, project_id, adapter_version, installed_at,
                   last_verified_at, status, metadata
                   FROM tool_installations WHERE tool_name = $1 AND project_id = $2"#,
            )
            .bind(tool_name)
            .bind(pid)
            .fetch_optional(self.pool)
            .await?
        } else {
            sqlx::query(
                r#"SELECT id, tool_name, project_id, adapter_version, installed_at,
                   last_verified_at, status, metadata
                   FROM tool_installations WHERE tool_name = $1 AND project_id IS NULL"#,
            )
            .bind(tool_name)
            .fetch_optional(self.pool)
            .await?
        };

        Ok(row.map(|r| ToolInstallationRow {
            id: r.get("id"),
            tool_name: r.get("tool_name"),
            project_id: r.get("project_id"),
            adapter_version: r.get("adapter_version"),
            installed_at: r.get("installed_at"),
            last_verified_at: r.get("last_verified_at"),
            status: r.get("status"),
            metadata: r.get("metadata"),
        }))
    }

    pub async fn upsert_component(&self, row: &InstalledComponentRow) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO installed_components (id, installation_id, component_type,
                component_slug, installed_version, installed_path, checksum, status, last_checked_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            ON CONFLICT (installation_id, component_slug) DO UPDATE SET
                installed_version = EXCLUDED.installed_version,
                installed_path = EXCLUDED.installed_path,
                checksum = EXCLUDED.checksum,
                status = EXCLUDED.status,
                last_checked_at = EXCLUDED.last_checked_at
            "#,
        )
        .bind(row.id)
        .bind(row.installation_id)
        .bind(&row.component_type)
        .bind(&row.component_slug)
        .bind(&row.installed_version)
        .bind(&row.installed_path)
        .bind(&row.checksum)
        .bind(&row.status)
        .bind(row.last_checked_at)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_components(
        &self,
        installation_id: Uuid,
    ) -> anyhow::Result<Vec<InstalledComponentRow>> {
        let rows = sqlx::query(
            r#"SELECT id, installation_id, component_type, component_slug,
               installed_version, installed_path, checksum, status, last_checked_at
               FROM installed_components WHERE installation_id = $1"#,
        )
        .bind(installation_id)
        .fetch_all(self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| InstalledComponentRow {
                id: r.get("id"),
                installation_id: r.get("installation_id"),
                component_type: r.get("component_type"),
                component_slug: r.get("component_slug"),
                installed_version: r.get("installed_version"),
                installed_path: r.get("installed_path"),
                checksum: r.get("checksum"),
                status: r.get("status"),
                last_checked_at: r.get("last_checked_at"),
            })
            .collect())
    }
}

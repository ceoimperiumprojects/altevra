use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::util::{opt_ts_from_text, opt_uuid_from_text, ts_from_text, ts_to_text, uuid_from_text};

#[derive(Debug, Clone)]
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

#[derive(Debug, Clone)]
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
    pool: &'a SqlitePool,
}

impl<'a> InstallationsRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn upsert_installation(&self, row: &ToolInstallationRow) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO tool_installations (id, tool_name, project_id, adapter_version,
                installed_at, last_verified_at, status, metadata)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT (tool_name, project_id) DO UPDATE SET
                adapter_version = excluded.adapter_version,
                last_verified_at = excluded.last_verified_at,
                status = excluded.status,
                metadata = excluded.metadata
            "#,
        )
        .bind(row.id.to_string())
        .bind(&row.tool_name)
        .bind(row.project_id.map(|u| u.to_string()))
        .bind(&row.adapter_version)
        .bind(ts_to_text(&row.installed_at))
        .bind(row.last_verified_at.as_ref().map(ts_to_text))
        .bind(&row.status)
        .bind(row.metadata.to_string())
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
                   FROM tool_installations WHERE tool_name = ? AND project_id = ?"#,
            )
            .bind(tool_name)
            .bind(pid.to_string())
            .fetch_optional(self.pool)
            .await?
        } else {
            sqlx::query(
                r#"SELECT id, tool_name, project_id, adapter_version, installed_at,
                   last_verified_at, status, metadata
                   FROM tool_installations WHERE tool_name = ? AND project_id IS NULL"#,
            )
            .bind(tool_name)
            .fetch_optional(self.pool)
            .await?
        };

        Ok(row.map(|r| ToolInstallationRow {
            id: uuid_from_text(r.get::<String, _>("id")),
            tool_name: r.get("tool_name"),
            project_id: opt_uuid_from_text(r.get::<Option<String>, _>("project_id")),
            adapter_version: r.get("adapter_version"),
            installed_at: ts_from_text(r.get::<String, _>("installed_at")),
            last_verified_at: opt_ts_from_text(r.get::<Option<String>, _>("last_verified_at")),
            status: r.get("status"),
            metadata: r
                .get::<Option<String>, _>("metadata")
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_else(|| serde_json::Value::Object(Default::default())),
        }))
    }

    pub async fn upsert_component(&self, row: &InstalledComponentRow) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO installed_components (id, installation_id, component_type,
                component_slug, installed_version, installed_path, checksum, status, last_checked_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT (installation_id, component_slug) DO UPDATE SET
                installed_version = excluded.installed_version,
                installed_path = excluded.installed_path,
                checksum = excluded.checksum,
                status = excluded.status,
                last_checked_at = excluded.last_checked_at
            "#,
        )
        .bind(row.id.to_string())
        .bind(row.installation_id.to_string())
        .bind(&row.component_type)
        .bind(&row.component_slug)
        .bind(&row.installed_version)
        .bind(&row.installed_path)
        .bind(&row.checksum)
        .bind(&row.status)
        .bind(row.last_checked_at.as_ref().map(ts_to_text))
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
               FROM installed_components WHERE installation_id = ?"#,
        )
        .bind(installation_id.to_string())
        .fetch_all(self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| InstalledComponentRow {
                id: uuid_from_text(r.get::<String, _>("id")),
                installation_id: uuid_from_text(r.get::<String, _>("installation_id")),
                component_type: r.get("component_type"),
                component_slug: r.get("component_slug"),
                installed_version: r.get("installed_version"),
                installed_path: r.get("installed_path"),
                checksum: r.get("checksum"),
                status: r.get("status"),
                last_checked_at: opt_ts_from_text(r.get::<Option<String>, _>("last_checked_at")),
            })
            .collect())
    }
}

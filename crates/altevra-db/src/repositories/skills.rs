use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::util::{ts_from_text, ts_to_text, uuid_from_text};

#[derive(Debug, Clone)]
pub struct SkillRow {
    pub id: Uuid,
    pub slug: String,
    pub version: String,
    pub source_path: String,
    pub checksum: String,
    pub content: String,
    pub metadata: serde_json::Value,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub struct SkillsRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> SkillsRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn upsert(&self, row: &SkillRow) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO skills (id, slug, version, source_path, checksum, content, metadata, status, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT (slug) DO UPDATE SET
                version = excluded.version,
                source_path = excluded.source_path,
                checksum = excluded.checksum,
                content = excluded.content,
                metadata = excluded.metadata,
                status = excluded.status,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(row.id.to_string())
        .bind(&row.slug)
        .bind(&row.version)
        .bind(&row.source_path)
        .bind(&row.checksum)
        .bind(&row.content)
        .bind(row.metadata.to_string())
        .bind(&row.status)
        .bind(ts_to_text(&row.created_at))
        .bind(ts_to_text(&row.updated_at))
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn find_by_slug(&self, slug: &str) -> anyhow::Result<Option<SkillRow>> {
        let row = sqlx::query(
            r#"SELECT id, slug, version, source_path, checksum, content, metadata, status,
               created_at, updated_at FROM skills WHERE slug = ?"#,
        )
        .bind(slug)
        .fetch_optional(self.pool)
        .await?;

        Ok(row.map(|r| SkillRow {
            id: uuid_from_text(r.get::<String, _>("id")),
            slug: r.get("slug"),
            version: r.get("version"),
            source_path: r.get("source_path"),
            checksum: r.get("checksum"),
            content: r.get("content"),
            metadata: r
                .get::<Option<String>, _>("metadata")
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_else(|| serde_json::Value::Object(Default::default())),
            status: r.get("status"),
            created_at: ts_from_text(r.get::<String, _>("created_at")),
            updated_at: ts_from_text(r.get::<String, _>("updated_at")),
        }))
    }

    pub async fn list_all(&self) -> anyhow::Result<Vec<SkillRow>> {
        let rows = sqlx::query(
            r#"SELECT id, slug, version, source_path, checksum, content, metadata, status,
               created_at, updated_at FROM skills ORDER BY slug"#,
        )
        .fetch_all(self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| SkillRow {
                id: uuid_from_text(r.get::<String, _>("id")),
                slug: r.get("slug"),
                version: r.get("version"),
                source_path: r.get("source_path"),
                checksum: r.get("checksum"),
                content: r.get("content"),
                metadata: r
                    .get::<Option<String>, _>("metadata")
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_else(|| serde_json::Value::Object(Default::default())),
                status: r.get("status"),
                created_at: ts_from_text(r.get::<String, _>("created_at")),
                updated_at: ts_from_text(r.get::<String, _>("updated_at")),
            })
            .collect())
    }
}

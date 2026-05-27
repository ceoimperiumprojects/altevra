use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

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
    pool: &'a PgPool,
}

impl<'a> SkillsRepository<'a> {
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }

    pub async fn upsert(&self, row: &SkillRow) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO skills (id, slug, version, source_path, checksum, content, metadata, status, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (slug) DO UPDATE SET
                version = EXCLUDED.version,
                source_path = EXCLUDED.source_path,
                checksum = EXCLUDED.checksum,
                content = EXCLUDED.content,
                metadata = EXCLUDED.metadata,
                status = EXCLUDED.status,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(row.id)
        .bind(&row.slug)
        .bind(&row.version)
        .bind(&row.source_path)
        .bind(&row.checksum)
        .bind(&row.content)
        .bind(&row.metadata)
        .bind(&row.status)
        .bind(row.created_at)
        .bind(row.updated_at)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn find_by_slug(&self, slug: &str) -> anyhow::Result<Option<SkillRow>> {
        let row = sqlx::query(
            r#"SELECT id, slug, version, source_path, checksum, content, metadata, status,
               created_at, updated_at FROM skills WHERE slug = $1"#,
        )
        .bind(slug)
        .fetch_optional(self.pool)
        .await?;

        Ok(row.map(|r| SkillRow {
            id: r.get("id"),
            slug: r.get("slug"),
            version: r.get("version"),
            source_path: r.get("source_path"),
            checksum: r.get("checksum"),
            content: r.get("content"),
            metadata: r.get("metadata"),
            status: r.get("status"),
            created_at: r.get("created_at"),
            updated_at: r.get("updated_at"),
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
                id: r.get("id"),
                slug: r.get("slug"),
                version: r.get("version"),
                source_path: r.get("source_path"),
                checksum: r.get("checksum"),
                content: r.get("content"),
                metadata: r.get("metadata"),
                status: r.get("status"),
                created_at: r.get("created_at"),
                updated_at: r.get("updated_at"),
            })
            .collect())
    }
}

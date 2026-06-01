//! Envelope-aware repositories for the P0.1 gap objects (BUILD_TASKS T1.12):
//! `learnings`, plus the cross-type `object_index` that the packet compiler
//! reads. Follows the existing concrete-struct repo pattern.

use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};

use crate::util::ts_to_text;

/// A durable learning row (envelope subset relevant to persistence).
#[derive(Debug, Clone)]
pub struct LearningRow {
    pub id: String,
    pub title: String,
    pub body: String,
    pub status: String,
    pub domain: String,
    pub scope: Option<String>,
    pub sensitivity: String,
    pub provenance: String,       // JSON
    pub redaction_status: String, // result of ingest_guard
    pub categories: String,       // JSON array
    pub tags: String,             // JSON array
    pub confidence: String,
}

impl LearningRow {
    /// Minimal constructor with safe envelope defaults.
    pub fn new(id: impl Into<String>, title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            body: body.into(),
            status: "active".into(),
            domain: "business".into(),
            scope: None,
            sensitivity: "internal".into(),
            provenance: "{\"origin\":\"pavle_direct\"}".into(),
            redaction_status: "clean".into(),
            categories: "[]".into(),
            tags: "[]".into(),
            confidence: "medium".into(),
        }
    }
}

pub struct LearningsRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> LearningsRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Insert a learning. Caller is responsible for having run `ingest_guard`
    /// first (TAG-1 / redaction enforced upstream).
    pub async fn insert(&self, row: &LearningRow) -> anyhow::Result<()> {
        let now = ts_to_text(&Utc::now());
        sqlx::query(
            "INSERT INTO learnings \
             (id, type, title, body, status, domain, scope, sensitivity, provenance, \
              redaction_status, categories, tags, confidence, created_at, updated_at) \
             VALUES (?, 'learning', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&row.id)
        .bind(&row.title)
        .bind(&row.body)
        .bind(&row.status)
        .bind(&row.domain)
        .bind(row.scope.as_deref())
        .bind(&row.sensitivity)
        .bind(&row.provenance)
        .bind(&row.redaction_status)
        .bind(&row.categories)
        .bind(&row.tags)
        .bind(&row.confidence)
        .bind(&now)
        .bind(&now)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn get(&self, id: &str) -> anyhow::Result<Option<LearningRow>> {
        let row = sqlx::query(
            "SELECT id, title, body, status, domain, scope, sensitivity, provenance, \
                    redaction_status, categories, tags, confidence \
             FROM learnings WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool)
        .await?;
        Ok(row.map(|r| LearningRow {
            id: r.get("id"),
            title: r.get("title"),
            body: r.get("body"),
            status: r.get("status"),
            domain: r.get("domain"),
            scope: r.get("scope"),
            sensitivity: r.get("sensitivity"),
            provenance: r.get("provenance"),
            redaction_status: r.get("redaction_status"),
            categories: r.get("categories"),
            tags: r.get("tags"),
            confidence: r.get("confidence"),
        }))
    }

    /// Default-readable learnings (status active/draft) for a domain.
    pub async fn list_active(&self, domain: &str) -> anyhow::Result<Vec<LearningRow>> {
        let rows = sqlx::query(
            "SELECT id, title, body, status, domain, scope, sensitivity, provenance, \
                    redaction_status, categories, tags, confidence \
             FROM learnings WHERE domain = ? AND status IN ('active','draft') \
             ORDER BY updated_at DESC",
        )
        .bind(domain)
        .fetch_all(self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| LearningRow {
                id: r.get("id"),
                title: r.get("title"),
                body: r.get("body"),
                status: r.get("status"),
                domain: r.get("domain"),
                scope: r.get("scope"),
                sensitivity: r.get("sensitivity"),
                provenance: r.get("provenance"),
                redaction_status: r.get("redaction_status"),
                categories: r.get("categories"),
                tags: r.get("tags"),
                confidence: r.get("confidence"),
            })
            .collect())
    }
}

/// A denormalized cross-type index row (the packet compiler's candidate source).
#[derive(Debug, Clone)]
pub struct ObjectIndexRow {
    pub object_type: String,
    pub id: String,
    pub status: String,
    pub sensitivity: String,
    pub domain: String,
    pub scope: Option<String>,
    pub title: Option<String>,
    pub categories: String,
    pub tags: String,
    pub updated_at: DateTime<Utc>,
}

pub struct ObjectIndexRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> ObjectIndexRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Upsert an index entry (maintained on every durable write).
    pub async fn upsert(&self, row: &ObjectIndexRow) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO object_index \
             (type, id, status, sensitivity, domain, scope, title, categories, tags, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&row.object_type)
        .bind(&row.id)
        .bind(&row.status)
        .bind(&row.sensitivity)
        .bind(&row.domain)
        .bind(row.scope.as_deref())
        .bind(row.title.as_deref())
        .bind(&row.categories)
        .bind(&row.tags)
        .bind(ts_to_text(&row.updated_at))
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Candidate rows for packet compilation, optionally filtered by domain.
    /// (The ExposureGate does the actual ceiling/scope filtering downstream.)
    pub async fn candidates(&self, domain: Option<&str>) -> anyhow::Result<Vec<ObjectIndexRow>> {
        let rows = if let Some(d) = domain {
            sqlx::query(
                "SELECT type, id, status, sensitivity, domain, scope, title, categories, tags, updated_at \
                 FROM object_index WHERE domain = ? ORDER BY updated_at DESC",
            )
            .bind(d)
            .fetch_all(self.pool)
            .await?
        } else {
            sqlx::query(
                "SELECT type, id, status, sensitivity, domain, scope, title, categories, tags, updated_at \
                 FROM object_index ORDER BY updated_at DESC",
            )
            .fetch_all(self.pool)
            .await?
        };
        Ok(rows
            .into_iter()
            .map(|r| ObjectIndexRow {
                object_type: r.get("type"),
                id: r.get("id"),
                status: r.get("status"),
                sensitivity: r.get("sensitivity"),
                domain: r.get("domain"),
                scope: r.get("scope"),
                title: r.get("title"),
                categories: r.get("categories"),
                tags: r.get("tags"),
                updated_at: crate::util::ts_from_text(r.get::<String, _>("updated_at")),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::{create_pool, run_migrations};

    async fn pool() -> SqlitePool {
        let p = create_pool("sqlite::memory:").await.unwrap();
        run_migrations(&p).await.unwrap();
        p
    }

    #[tokio::test]
    async fn learning_roundtrip_preserves_envelope() {
        let p = pool().await;
        let repo = LearningsRepository::new(&p);
        let mut row = LearningRow::new("l1", "Late nights hurt focus", "## Learning\nbody");
        row.domain = "health".into();
        row.sensitivity = "restricted".into();
        row.categories = "[\"health\"]".into();
        repo.insert(&row).await.unwrap();

        let got = repo.get("l1").await.unwrap().expect("row exists");
        assert_eq!(got.domain, "health");
        assert_eq!(got.sensitivity, "restricted");
        assert_eq!(got.categories, "[\"health\"]");
        assert_eq!(repo.list_active("health").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn object_index_candidates_roundtrip() {
        let p = pool().await;
        let idx = ObjectIndexRepository::new(&p);
        idx.upsert(&ObjectIndexRow {
            object_type: "decision".into(),
            id: "d1".into(),
            status: "active".into(),
            sensitivity: "internal".into(),
            domain: "business".into(),
            scope: None,
            title: Some("A decision".into()),
            categories: "[\"gtm\"]".into(),
            tags: "[]".into(),
            updated_at: Utc::now(),
        })
        .await
        .unwrap();

        assert_eq!(idx.candidates(None).await.unwrap().len(), 1);
        assert_eq!(idx.candidates(Some("business")).await.unwrap().len(), 1);
        assert!(idx.candidates(Some("health")).await.unwrap().is_empty());
    }
}

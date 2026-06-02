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

        // T-INV14: a written learning immediately becomes a packet candidate +
        // full-text searchable. The single index maintenance point keeps
        // object_index + object_fts in sync with the durable write.
        ObjectIndexRepository::new(self.pool)
            .index_object(
                &ObjectIndexRow {
                    object_type: "learning".into(),
                    id: row.id.clone(),
                    status: row.status.clone(),
                    sensitivity: row.sensitivity.clone(),
                    domain: row.domain.clone(),
                    scope: row.scope.clone(),
                    title: Some(row.title.clone()),
                    categories: row.categories.clone(),
                    tags: row.tags.clone(),
                    redaction_status: row.redaction_status.clone(),
                    updated_at: Utc::now(),
                },
                &row.body,
            )
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
    /// Redaction verdict from `ingest_guard` — the exposure gate fails closed on
    /// anything other than `clean`/`redacted` (R11: was missing → fail-open).
    pub redaction_status: String,
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
             (type, id, status, sensitivity, domain, scope, title, categories, tags, redaction_status, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        .bind(&row.redaction_status)
        .bind(ts_to_text(&row.updated_at))
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// The SINGLE write-time maintenance point: keep BOTH the structured index
    /// (`object_index` — the packet compiler's candidate source) AND the FTS
    /// substrate (`object_fts` — the bm25 signal, R12) in sync (T1.13 + T1.14b).
    /// Durable writers call this so retrieval candidates and full-text stay
    /// consistent; the ExposureGate still gates exposure downstream.
    pub async fn index_object(&self, row: &ObjectIndexRow, body: &str) -> anyhow::Result<()> {
        self.upsert(row).await?;
        crate::repositories::fts::FtsRepository::new(self.pool)
            .index(
                &row.object_type,
                &row.id,
                row.title.as_deref().unwrap_or(""),
                body,
                &row.tags,
            )
            .await?;
        Ok(())
    }

    /// RTBF soft-forget (P0.8 T8.6): mark the object `forgotten` in the index and
    /// drop it from the FTS substrate so it is no longer retrievable/searchable.
    /// Soft (status flip), not a hard wipe — the caller is human-presence gated.
    /// Returns true if a row was affected.
    pub async fn forget(&self, object_type: &str, id: &str) -> anyhow::Result<bool> {
        let res =
            sqlx::query("UPDATE object_index SET status = 'forgotten' WHERE type = ? AND id = ?")
                .bind(object_type)
                .bind(id)
                .execute(self.pool)
                .await?;
        // Remove from full-text so a forgotten object can't surface via search.
        sqlx::query("DELETE FROM object_fts WHERE object_type = ? AND object_id = ?")
            .bind(object_type)
            .bind(id)
            .execute(self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Candidate rows for packet compilation, optionally filtered by domain.
    /// (The ExposureGate does the actual ceiling/scope filtering downstream.)
    pub async fn candidates(&self, domain: Option<&str>) -> anyhow::Result<Vec<ObjectIndexRow>> {
        let rows = if let Some(d) = domain {
            sqlx::query(
                "SELECT type, id, status, sensitivity, domain, scope, title, categories, tags, redaction_status, updated_at \
                 FROM object_index WHERE domain = ? ORDER BY updated_at DESC",
            )
            .bind(d)
            .fetch_all(self.pool)
            .await?
        } else {
            sqlx::query(
                "SELECT type, id, status, sensitivity, domain, scope, title, categories, tags, redaction_status, updated_at \
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
                redaction_status: r.get("redaction_status"),
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

        // T-INV14: the write populated the retrieval substrate automatically —
        // a packet candidate (structured) + full-text searchable (bm25).
        let idx = ObjectIndexRepository::new(&p);
        assert_eq!(idx.candidates(Some("health")).await.unwrap().len(), 1);
        let fts = crate::repositories::fts::FtsRepository::new(&p);
        assert!(fts
            .search("Late nights focus", 10)
            .await
            .unwrap()
            .iter()
            .any(|h| h.object_id == "l1"));
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
            redaction_status: "clean".into(),
            updated_at: Utc::now(),
        })
        .await
        .unwrap();

        assert_eq!(idx.candidates(None).await.unwrap().len(), 1);
        assert_eq!(idx.candidates(Some("business")).await.unwrap().len(), 1);
        assert!(idx.candidates(Some("health")).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn index_object_maintains_both_index_and_fts() {
        let p = pool().await;
        let idx = ObjectIndexRepository::new(&p);
        idx.index_object(
            &ObjectIndexRow {
                object_type: "decision".into(),
                id: "d9".into(),
                status: "active".into(),
                sensitivity: "internal".into(),
                domain: "project".into(),
                scope: None,
                title: Some("Adopt SQLite for local-first storage".into()),
                categories: "[\"storage\"]".into(),
                tags: "[\"storage\",\"db\"]".into(),
                redaction_status: "clean".into(),
                updated_at: Utc::now(),
            },
            "We adopt SQLite as the canonical local-first store; embeddings stay optional.",
        )
        .await
        .unwrap();
        // structured index sees it (packet candidate source)...
        assert_eq!(idx.candidates(Some("project")).await.unwrap().len(), 1);
        // ...and the FTS substrate finds it by a body term (bm25 signal).
        let fts = crate::repositories::fts::FtsRepository::new(&p);
        let hits = fts.search("SQLite local-first", 10).await.unwrap();
        assert!(
            hits.iter().any(|h| h.object_id == "d9"),
            "FTS must find indexed object"
        );
    }

    #[tokio::test]
    async fn forget_soft_marks_and_drops_from_fts() {
        let p = pool().await;
        let idx = ObjectIndexRepository::new(&p);
        idx.index_object(
            &ObjectIndexRow {
                object_type: "learning".into(),
                id: "f1".into(),
                status: "active".into(),
                sensitivity: "internal".into(),
                domain: "business".into(),
                scope: None,
                title: Some("forget me".into()),
                categories: "[]".into(),
                tags: "[]".into(),
                redaction_status: "clean".into(),
                updated_at: Utc::now(),
            },
            "secret-ish body to forget",
        )
        .await
        .unwrap();
        let fts = crate::repositories::fts::FtsRepository::new(&p);
        assert_eq!(fts.search("forget", 10).await.unwrap().len(), 1);

        assert!(idx.forget("learning", "f1").await.unwrap());
        // dropped from search...
        assert!(fts.search("forget", 10).await.unwrap().is_empty());
        // ...and the index row is now status=forgotten (soft, not wiped).
        let row = sqlx::query("SELECT status FROM object_index WHERE id = 'f1'")
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(row.get::<String, _>("status"), "forgotten");
        // forgetting an unknown object is a no-op.
        assert!(!idx.forget("learning", "nope").await.unwrap());
    }
}

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

    /// Insert (or replace) a learning. Caller is responsible for having run
    /// `ingest_guard` first (TAG-1 / redaction enforced upstream).
    ///
    /// `INSERT OR REPLACE` (not plain INSERT) so re-capturing the same content-id is
    /// idempotent — the incremental re-atomize path (`capture --watch`) re-writes an
    /// unchanged section's id without a UNIQUE violation; a changed section gets a
    /// new id and the stale one is `forget`-ten by the caller.
    pub async fn insert(&self, row: &LearningRow) -> anyhow::Result<()> {
        let now = ts_to_text(&Utc::now());
        sqlx::query(
            "INSERT OR REPLACE INTO learnings \
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

/// A durable insight-card row (DB-only synthesized insight, migration 020).
///
/// Mirrors [`LearningRow`]'s envelope subset. The default provenance is
/// `agent_inferred` (an insight is synthesized, not a direct Pavle statement)
/// and the default confidence `medium`.
#[derive(Debug, Clone)]
pub struct InsightCardRow {
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

impl InsightCardRow {
    /// Minimal constructor with safe envelope defaults (agent-inferred provenance).
    pub fn new(id: impl Into<String>, title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            body: body.into(),
            status: "active".into(),
            domain: "business".into(),
            scope: None,
            sensitivity: "internal".into(),
            provenance: "{\"origin\":\"agent_inferred\"}".into(),
            redaction_status: "clean".into(),
            categories: "[]".into(),
            tags: "[]".into(),
            confidence: "medium".into(),
        }
    }
}

pub struct InsightCardsRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> InsightCardsRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Insert (or replace) an insight card. `INSERT OR REPLACE` so re-synthesizing
    /// the same content-id is idempotent. Like [`LearningsRepository::insert`], the
    /// write immediately populates the retrieval substrate (`object_index` +
    /// `object_fts`) via [`ObjectIndexRepository::index_object`] (A1 / T-INV14), so
    /// `recall` finds the card. Caller is responsible for `ingest_guard` upstream
    /// (TAG-1 / redaction).
    pub async fn insert(&self, row: &InsightCardRow) -> anyhow::Result<()> {
        let now = ts_to_text(&Utc::now());
        sqlx::query(
            "INSERT OR REPLACE INTO insight_cards \
             (id, type, title, body, status, domain, scope, sensitivity, provenance, \
              redaction_status, categories, tags, confidence, created_at, updated_at) \
             VALUES (?, 'insight_card', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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

        // A1 / T-INV14: a written card is immediately a packet candidate + full-text
        // searchable (so `recall` finds it). Same single index-maintenance point as
        // every other durable write.
        ObjectIndexRepository::new(self.pool)
            .index_object(
                &ObjectIndexRow {
                    object_type: "insight_card".into(),
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

    pub async fn get(&self, id: &str) -> anyhow::Result<Option<InsightCardRow>> {
        let row = sqlx::query(
            "SELECT id, title, body, status, domain, scope, sensitivity, provenance, \
                    redaction_status, categories, tags, confidence \
             FROM insight_cards WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool)
        .await?;
        Ok(row.map(|r| InsightCardRow {
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

    /// Count of insight cards (for quick verification/reporting).
    pub async fn count(&self) -> anyhow::Result<i64> {
        let row = sqlx::query("SELECT COUNT(*) AS n FROM insight_cards")
            .fetch_one(self.pool)
            .await?;
        Ok(row.get::<i64, _>("n"))
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

    /// E1 — soft-archive the index row (Active → Archived). Status only; the
    /// row stays and the FTS substrate stays (an archived object can still be
    /// recalled with status=archived predicates downstream). Returns true if a
    /// row was affected. Skips rows under legal hold (D7).
    pub async fn archive(&self, object_type: &str, id: &str) -> anyhow::Result<bool> {
        let res = sqlx::query(
            "UPDATE object_index \
             SET status = 'archived', updated_at = ? \
             WHERE type = ? AND id = ? AND status = 'active' AND legal_hold = 0",
        )
        .bind(ts_to_text(&Utc::now()))
        .bind(object_type)
        .bind(id)
        .execute(self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    /// E1 — set the per-object legal-hold flag (D7). A held object is never
    /// purged or auto-archived by the lifecycle sweep. The destructive forget
    /// path also consults this flag downstream.
    pub async fn set_legal_hold(
        &self,
        object_type: &str,
        id: &str,
        held: bool,
    ) -> anyhow::Result<bool> {
        let res = sqlx::query(
            "UPDATE object_index SET legal_hold = ? WHERE type = ? AND id = ?",
        )
        .bind(if held { 1i64 } else { 0i64 })
        .bind(object_type)
        .bind(id)
        .execute(self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    /// E1 — read the per-object legal-hold flag.
    pub async fn is_legal_held(&self, object_type: &str, id: &str) -> anyhow::Result<bool> {
        let row = sqlx::query(
            "SELECT legal_hold FROM object_index WHERE type = ? AND id = ?",
        )
        .bind(object_type)
        .bind(id)
        .fetch_optional(self.pool)
        .await?;
        Ok(row.map(|r| r.get::<i64, _>("legal_hold") != 0).unwrap_or(false))
    }

    /// E1 — set the soft lifecycle marker (e.g. `"pending_delete"`) on an
    /// `object_index` row. Surfaces delete-due objects in Pavle's digest
    /// WITHOUT actually deleting — the destructive forget remains
    /// presence-gated (R4). `marker=None` clears the column.
    pub async fn set_lifecycle_marker(
        &self,
        object_type: &str,
        id: &str,
        marker: Option<&str>,
    ) -> anyhow::Result<bool> {
        let res = sqlx::query(
            "UPDATE object_index SET lifecycle_marker = ? WHERE type = ? AND id = ?",
        )
        .bind(marker)
        .bind(object_type)
        .bind(id)
        .execute(self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    /// IDs of non-forgotten index rows whose id begins with `prefix`. Used by the
    /// incremental re-atomize path (`capture --watch`) to find the prior objects
    /// derived from one file (all share the `capture-<filestem>-` id prefix) so the
    /// stale ones can be `forget`-ten when a living doc is edited. `%`/`_` in the
    /// prefix are escaped so a filestem with those chars can't widen the match.
    pub async fn ids_with_prefix(&self, prefix: &str) -> anyhow::Result<Vec<(String, String)>> {
        let escaped = prefix
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let like = format!("{escaped}%");
        let rows = sqlx::query(
            "SELECT type, id FROM object_index \
             WHERE id LIKE ? ESCAPE '\\' AND status != 'forgotten'",
        )
        .bind(like)
        .fetch_all(self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get::<String, _>("type"), r.get::<String, _>("id")))
            .collect())
    }

    /// Non-forgotten index rows whose `categories` is still the empty array `[]`
    /// (no resolved category yet) — the AutoCategorizer's work queue (B5,
    /// CLAUDE.md §3.2). Newest first; `limit` caps a batch.
    pub async fn uncategorized(&self, limit: i64) -> anyhow::Result<Vec<ObjectIndexRow>> {
        let rows = sqlx::query(
            "SELECT type, id, status, sensitivity, domain, scope, title, categories, tags, redaction_status, updated_at \
             FROM object_index \
             WHERE status != 'forgotten' AND TRIM(categories) IN ('[]', '') \
             ORDER BY updated_at DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(self.pool)
        .await?;
        Ok(rows.into_iter().map(row_to_index).collect())
    }

    /// The set of distinct categories already in use across the index (the living
    /// taxonomy the classifier matches against). Flattens each row's JSON
    /// `categories` array; sorted + deduped. Empty arrays contribute nothing.
    pub async fn distinct_categories(&self) -> anyhow::Result<Vec<String>> {
        let rows = sqlx::query(
            "SELECT DISTINCT categories FROM object_index WHERE status != 'forgotten'",
        )
        .fetch_all(self.pool)
        .await?;
        let mut out: Vec<String> = Vec::new();
        for r in rows {
            let raw: String = r.get("categories");
            if let Ok(cats) = serde_json::from_str::<Vec<String>>(&raw) {
                for c in cats {
                    let c = c.trim().to_string();
                    if !c.is_empty() && !out.contains(&c) {
                        out.push(c);
                    }
                }
            }
        }
        out.sort();
        Ok(out)
    }

    /// Tag one object with a resolved category (B5). Sets `object_index.categories`
    /// to the JSON array `[category]` and bumps `updated_at`. Does NOT touch the FTS
    /// body (the category is index metadata, not searchable prose). Returns true if
    /// a row was updated.
    pub async fn set_category(
        &self,
        object_type: &str,
        id: &str,
        category: &str,
    ) -> anyhow::Result<bool> {
        let cats = serde_json::to_string(&vec![category])?;
        let now = ts_to_text(&Utc::now());
        let res = sqlx::query(
            "UPDATE object_index SET categories = ?, updated_at = ? WHERE type = ? AND id = ?",
        )
        .bind(&cats)
        .bind(&now)
        .bind(object_type)
        .bind(id)
        .execute(self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    /// The parsed `categories` array for one object (empty if absent/unparseable).
    /// Convenience for verifying a tag write (B5).
    pub async fn get_categories_or_empty(&self, object_type: &str, id: &str) -> Vec<String> {
        let row = sqlx::query("SELECT categories FROM object_index WHERE type = ? AND id = ?")
            .bind(object_type)
            .bind(id)
            .fetch_optional(self.pool)
            .await
            .ok()
            .flatten();
        row.and_then(|r| serde_json::from_str::<Vec<String>>(&r.get::<String, _>("categories")).ok())
            .unwrap_or_default()
    }

    /// E1 — every index row PLUS its `legal_hold` flag, for the lifecycle
    /// sweep. Reuses the same row shape as `candidates`; callers that need
    /// the hold bit call this. Forgotten rows are excluded — they already
    /// passed the destructive seam.
    pub async fn iter_for_lifecycle(&self) -> anyhow::Result<Vec<(ObjectIndexRow, bool)>> {
        let rows = sqlx::query(
            "SELECT type, id, status, sensitivity, domain, scope, title, categories, tags, redaction_status, updated_at, legal_hold \
             FROM object_index WHERE status != 'forgotten'",
        )
        .fetch_all(self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                let held: i64 = r.try_get("legal_hold").unwrap_or(0);
                (row_to_index(r), held != 0)
            })
            .collect())
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
        Ok(rows.into_iter().map(row_to_index).collect())
    }
}

fn row_to_index(r: sqlx::sqlite::SqliteRow) -> ObjectIndexRow {
    ObjectIndexRow {
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

    #[tokio::test]
    async fn ids_with_prefix_finds_file_objects_excludes_forgotten() {
        let p = pool().await;
        let idx = ObjectIndexRepository::new(&p);
        for id in ["capture-decisions-a-1111", "capture-decisions-b-2222"] {
            idx.index_object(
                &ObjectIndexRow {
                    object_type: "learning".into(),
                    id: id.into(),
                    status: "active".into(),
                    sensitivity: "internal".into(),
                    domain: "business".into(),
                    scope: None,
                    title: Some(id.into()),
                    categories: "[]".into(),
                    tags: "[]".into(),
                    redaction_status: "clean".into(),
                    updated_at: Utc::now(),
                },
                "body",
            )
            .await
            .unwrap();
        }
        // a different file's object must NOT match.
        idx.index_object(
            &ObjectIndexRow {
                object_type: "learning".into(),
                id: "capture-people-x-3333".into(),
                status: "active".into(),
                sensitivity: "internal".into(),
                domain: "relationship".into(),
                scope: None,
                title: Some("other".into()),
                categories: "[]".into(),
                tags: "[]".into(),
                redaction_status: "clean".into(),
                updated_at: Utc::now(),
            },
            "body",
        )
        .await
        .unwrap();

        let mut got = idx.ids_with_prefix("capture-decisions-").await.unwrap();
        got.sort();
        assert_eq!(
            got,
            vec![
                (
                    "learning".to_string(),
                    "capture-decisions-a-1111".to_string()
                ),
                (
                    "learning".to_string(),
                    "capture-decisions-b-2222".to_string()
                ),
            ]
        );

        // forgotten rows are excluded.
        idx.forget("learning", "capture-decisions-a-1111")
            .await
            .unwrap();
        let after = idx.ids_with_prefix("capture-decisions-").await.unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].1, "capture-decisions-b-2222");
    }
}

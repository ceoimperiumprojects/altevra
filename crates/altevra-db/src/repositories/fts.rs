//! FTS5 lexical retrieval (P0.4 / R12 T1.14b) — the PRIMARY full-text substrate
//! (BM25), NO vectors. Maintained alongside `object_index` on durable writes;
//! the packet compiler ranks survivors by `bm25 + tag_match + graph + recency`,
//! and this provides the bm25 signal deterministically with no model.

use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};

#[derive(Debug, Clone)]
pub struct FtsHit {
    pub object_type: String,
    pub object_id: String,
    pub title: String,
    /// FTS5 bm25 score — LOWER is a better match (sqlite bm25 returns negatives
    /// for better matches); we expose it raw and sort ascending.
    pub score: f64,
}

/// A FTS hit enriched with `object_index` metadata (domain, sensitivity,
/// updated_at) + the indexed body — everything `recall` needs to render a
/// provenance breadcrumb for a durable object (decision/learning/wiki/…) the
/// same way it does for session turns. `redaction_status` is carried so callers
/// can fail-closed on un-redacted objects (R11).
#[derive(Debug, Clone)]
pub struct ObjectHit {
    pub object_type: String,
    pub object_id: String,
    pub title: String,
    pub body: String,
    pub domain: String,
    pub sensitivity: String,
    pub redaction_status: String,
    /// JSON array of category/tag strings from `object_index`. Carries the
    /// `kind:<type>` tag for atomized objects (every captured section is stored
    /// as a `learning` row, so the *real* type — decision/person/note — lives
    /// here). Lets `recall` label a captured decision as a decision, not a
    /// generic learning, matching the `--with` entity path.
    pub tags: String,
    pub updated_at: DateTime<Utc>,
    pub score: f64,
}

pub struct FtsRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> FtsRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Upsert an object's searchable text. Re-indexing deletes the old row first
    /// (FTS5 has no UPSERT) so a re-write doesn't duplicate.
    pub async fn index(
        &self,
        object_type: &str,
        object_id: &str,
        title: &str,
        body: &str,
        tags: &str,
    ) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM object_fts WHERE object_type = ? AND object_id = ?")
            .bind(object_type)
            .bind(object_id)
            .execute(self.pool)
            .await?;
        sqlx::query(
            "INSERT INTO object_fts (object_type, object_id, title, body, tags) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(object_type)
        .bind(object_id)
        .bind(title)
        .bind(body)
        .bind(tags)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// BM25 full-text search. Returns hits best-first. `query` is an FTS5 MATCH
    /// expression; callers pass plain terms (FTS5 ANDs them).
    pub async fn search(&self, query: &str, limit: i64) -> anyhow::Result<Vec<FtsHit>> {
        // Sanitize: FTS5 MATCH treats some chars as operators. Quote each term so
        // arbitrary user text is a safe phrase/term query.
        let safe = sanitize_query(query);
        if safe.is_empty() {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(
            "SELECT object_type, object_id, title, bm25(object_fts) AS score \
             FROM object_fts WHERE object_fts MATCH ? ORDER BY score LIMIT ?",
        )
        .bind(&safe)
        .bind(limit)
        .fetch_all(self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| FtsHit {
                object_type: r.get("object_type"),
                object_id: r.get("object_id"),
                title: r.get("title"),
                score: r.get("score"),
            })
            .collect())
    }

    /// BM25 search that returns the body + `object_index` metadata in one query —
    /// the durable-object half of unified recall (decisions/learnings/wiki/…).
    /// Joins `object_fts` (which holds the body) with `object_index` (domain,
    /// sensitivity, updated_at). Best-first (ascending bm25).
    pub async fn search_objects(&self, query: &str, limit: i64) -> anyhow::Result<Vec<ObjectHit>> {
        let safe = sanitize_query(query);
        if safe.is_empty() {
            return Ok(Vec::new());
        }
        // Step 1: FTS match (the proven non-aliased pattern; +body which object_fts
        // stores). FTS5 MATCH inside a JOIN is finicky across versions, so we keep
        // the match query pure and enrich metadata in step 2.
        let fts_rows = sqlx::query(
            "SELECT object_type, object_id, title, body, bm25(object_fts) AS score \
             FROM object_fts WHERE object_fts MATCH ? ORDER BY score LIMIT ?",
        )
        .bind(&safe)
        .bind(limit)
        .fetch_all(self.pool)
        .await?;

        // Step 2: enrich each hit with object_index metadata (domain, sensitivity,
        // redaction_status, updated_at). Fail-soft defaults if the index row is
        // missing (shouldn't happen — index_object writes both atomically).
        let mut out = Vec::with_capacity(fts_rows.len());
        for r in fts_rows {
            let object_type: String = r.get("object_type");
            let object_id: String = r.get("object_id");
            let meta = sqlx::query(
                "SELECT domain, sensitivity, redaction_status, tags, updated_at \
                 FROM object_index WHERE type = ? AND id = ?",
            )
            .bind(&object_type)
            .bind(&object_id)
            .fetch_optional(self.pool)
            .await?;
            let (domain, sensitivity, redaction_status, tags, updated_at) = match meta {
                Some(m) => (
                    m.get::<String, _>("domain"),
                    m.get::<String, _>("sensitivity"),
                    m.get::<String, _>("redaction_status"),
                    m.get::<String, _>("tags"),
                    crate::util::opt_ts_from_text(m.get("updated_at")).unwrap_or_else(Utc::now),
                ),
                None => (
                    "?".into(),
                    "internal".into(),
                    "clean".into(),
                    "[]".into(),
                    Utc::now(),
                ),
            };
            out.push(ObjectHit {
                object_type,
                object_id,
                title: r.get("title"),
                body: r.get("body"),
                domain,
                sensitivity,
                redaction_status,
                tags,
                updated_at,
                score: r.get("score"),
            });
        }
        Ok(out)
    }
}

/// Turn free text into a safe FTS5 MATCH expression: each alphanumeric token is
/// double-quoted (so punctuation/operators can't inject), joined by space (AND).
fn sanitize_query(q: &str) -> String {
    q.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\""))
        .collect::<Vec<_>>()
        .join(" ")
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
    async fn fts_ranks_relevant_first() {
        let p = pool().await;
        let repo = FtsRepository::new(&p);
        repo.index(
            "decision",
            "d1",
            "SQLite storage decision",
            "We adopt SQLite as the local-first store",
            "storage,db",
        )
        .await
        .unwrap();
        repo.index(
            "decision",
            "d2",
            "GTM plan",
            "Cold-call outreach to surplus operators",
            "gtm,sales",
        )
        .await
        .unwrap();
        let hits = repo.search("SQLite storage", 10).await.unwrap();
        assert!(!hits.is_empty(), "expected a match");
        assert_eq!(hits[0].object_id, "d1", "storage doc must rank first");
        // a non-matching query returns nothing.
        assert!(repo
            .search("kubernetes helm chart", 10)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn reindex_does_not_duplicate() {
        let p = pool().await;
        let repo = FtsRepository::new(&p);
        repo.index("wiki", "w1", "first", "alpha beta", "x")
            .await
            .unwrap();
        repo.index("wiki", "w1", "second", "alpha gamma", "x")
            .await
            .unwrap();
        let hits = repo.search("alpha", 10).await.unwrap();
        assert_eq!(hits.len(), 1, "re-index must replace, not duplicate");
        assert_eq!(hits[0].title, "second");
    }

    #[tokio::test]
    async fn empty_query_is_safe() {
        let p = pool().await;
        let repo = FtsRepository::new(&p);
        assert!(repo.search("", 10).await.unwrap().is_empty());
        assert!(repo.search("   !!! ", 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn search_objects_returns_body_and_metadata() {
        // Unified-recall path: a learning indexed via LearningsRepository becomes
        // searchable WITH its body + domain + updated_at (joined from object_index).
        let p = pool().await;
        let learnings = crate::repositories::objects::LearningsRepository::new(&p);
        let mut row = crate::repositories::objects::LearningRow::new(
            "L1",
            "GTM Decision",
            "Target Florida surplus buyers for ReVesta",
        );
        row.domain = "business".into();
        learnings.insert(&row).await.unwrap();

        let repo = FtsRepository::new(&p);
        let hits = repo.search_objects("Florida surplus", 10).await.unwrap();
        assert_eq!(hits.len(), 1, "the learning is found via object_fts");
        let h = &hits[0];
        assert_eq!(h.object_type, "learning");
        assert_eq!(h.object_id, "L1");
        assert!(h.body.contains("Florida surplus"), "body is returned");
        assert_eq!(h.domain, "business", "domain joined from object_index");
        assert_eq!(h.redaction_status, "clean");
    }
}

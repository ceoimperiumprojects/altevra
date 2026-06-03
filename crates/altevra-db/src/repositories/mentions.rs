//! Mention graph over the existing `relations` edge table (migration 020).
//!
//! When a captured object or turn mentions a known person/project (detected by
//! `altevra_core::entity::detect_mentions`), we record a typed edge
//! `mentions(from=object, to=entity)`. This is the cross-link substrate behind
//! "what did I do with Đorđe this month" (vision §4.1) and the "haven't talked to
//! X in N weeks" proactive seed (§3.6).
//!
//! Edges are idempotent (the `relations` UNIQUE(from,rel,to) constraint +
//! INSERT OR IGNORE), local SQLite only (SI-7 unaffected). Re-atomizing a file
//! REPLACES its edges: the caller `clear_from` the object's edges, then records
//! the current mention set — mirroring how the object reconcile forgets stale
//! sections.

use sqlx::{Row, SqlitePool};
use uuid::Uuid;

/// `rel` predicate value for a mention edge.
const REL_MENTIONS: &str = "mentions";

/// One mention edge row (the subset we read back).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MentionEdge {
    pub from_type: String,
    pub from_id: String,
    /// `person` | `project`.
    pub to_type: String,
    /// The entity id, e.g. `person:djordje`.
    pub to_id: String,
}

pub struct MentionsRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> MentionsRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Record a `mentions` edge from an object/turn to an entity. Idempotent:
    /// the `relations` UNIQUE(from_type,from_id,rel,to_type,to_id,to_ref) +
    /// `INSERT OR IGNORE` means recording the same edge twice is a no-op (no dup).
    /// Returns true if a NEW edge row was inserted.
    pub async fn record(
        &self,
        from_type: &str,
        from_id: &str,
        to_type: &str,
        entity_id: &str,
    ) -> anyhow::Result<bool> {
        // `to_ref` is bound to '' (not NULL): the relations UNIQUE constraint
        // includes to_ref, and SQLite treats NULLs as DISTINCT, so a NULL to_ref
        // would defeat dedup. An empty-string sentinel makes the edge identity
        // (from,rel,to_type,to_id) truly unique → idempotent INSERT OR IGNORE.
        let res = sqlx::query(
            "INSERT OR IGNORE INTO relations \
             (id, from_type, from_id, rel, to_type, to_id, to_ref, provenance, status) \
             VALUES (?, ?, ?, ?, ?, ?, '', '{\"origin\":\"system_derived\",\"by\":\"entity_extraction\"}', 'active')",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(from_type)
        .bind(from_id)
        .bind(REL_MENTIONS)
        .bind(to_type)
        .bind(entity_id)
        .execute(self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Remove all `mentions` edges originating from one object/turn. Used before a
    /// re-atomize so a file's edge set exactly reflects its current content (a
    /// section whose mention disappeared loses its edge).
    pub async fn clear_from(&self, from_type: &str, from_id: &str) -> anyhow::Result<u64> {
        let res =
            sqlx::query("DELETE FROM relations WHERE rel = ? AND from_type = ? AND from_id = ?")
                .bind(REL_MENTIONS)
                .bind(from_type)
                .bind(from_id)
                .execute(self.pool)
                .await?;
        Ok(res.rows_affected())
    }

    /// Remove all `mentions` edges from objects whose `from_id` begins with
    /// `prefix` (e.g. `capture-<filestem>-` — every section of one living doc).
    /// `%`/`_` escaped so a filestem with those chars can't widen the match.
    pub async fn clear_from_prefix(&self, prefix: &str) -> anyhow::Result<u64> {
        let escaped = prefix
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let like = format!("{escaped}%");
        let res = sqlx::query("DELETE FROM relations WHERE rel = ? AND from_id LIKE ? ESCAPE '\\'")
            .bind(REL_MENTIONS)
            .bind(like)
            .execute(self.pool)
            .await?;
        Ok(res.rows_affected())
    }

    /// All object/turn ids that mention `entity_id` (most recent first by the
    /// object's `object_index.updated_at` when joinable, else edge creation order).
    /// Returns `(from_type, from_id)` pairs — the caller resolves bodies/titles.
    pub async fn objects_mentioning(
        &self,
        entity_id: &str,
        limit: i64,
    ) -> anyhow::Result<Vec<(String, String)>> {
        let rows = sqlx::query(
            "SELECT r.from_type AS ft, r.from_id AS fid \
             FROM relations r \
             LEFT JOIN object_index oi ON oi.type = r.from_type AND oi.id = r.from_id \
             WHERE r.rel = ? AND r.to_id = ? AND r.status = 'active' \
             ORDER BY COALESCE(oi.updated_at, r.created_at) DESC \
             LIMIT ?",
        )
        .bind(REL_MENTIONS)
        .bind(entity_id)
        .bind(limit)
        .fetch_all(self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get::<String, _>("ft"), r.get::<String, _>("fid")))
            .collect())
    }

    /// Dated mention rows for feeding `altevra_core::last_contact`: every active
    /// `mentions` edge as `(vec![entity_id], date)` where `date` is the mentioning
    /// object's `object_index.updated_at` (NULL when the object isn't indexed). This
    /// is the substrate for the "haven't talked to X in N weeks" briefing line
    /// (CLAUDE.md §3.6) — the caller passes the whole set to `last_contact` per
    /// entity, which picks the most-recent date.
    pub async fn dated_mentions(&self) -> anyhow::Result<Vec<(Vec<String>, Option<chrono::NaiveDate>)>> {
        let rows = sqlx::query(
            "SELECT r.to_id AS eid, oi.updated_at AS up \
             FROM relations r \
             LEFT JOIN object_index oi ON oi.type = r.from_type AND oi.id = r.from_id \
             WHERE r.rel = ? AND r.status = 'active'",
        )
        .bind(REL_MENTIONS)
        .fetch_all(self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| {
                let eid: String = r.get("eid");
                let up: Option<String> = r.get("up");
                let date = up.and_then(|s| {
                    // `object_index.updated_at` is canonical RFC3339; take the date.
                    chrono::DateTime::parse_from_rfc3339(&s)
                        .ok()
                        .map(|d| d.date_naive())
                });
                (vec![eid], date)
            })
            .collect())
    }

    /// Count of distinct objects mentioning each entity — for a quick graph
    /// summary. Returns `(entity_id, count)` sorted by count desc.
    pub async fn mention_counts(&self) -> anyhow::Result<Vec<(String, i64)>> {
        let rows = sqlx::query(
            "SELECT to_id, COUNT(DISTINCT from_id) AS n FROM relations \
             WHERE rel = ? AND status = 'active' GROUP BY to_id ORDER BY n DESC",
        )
        .bind(REL_MENTIONS)
        .fetch_all(self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get::<String, _>("to_id"), r.get::<i64, _>("n")))
            .collect())
    }

    /// Distinct entity ids mentioned by one object (for verification/reporting).
    pub async fn entities_for(
        &self,
        from_type: &str,
        from_id: &str,
    ) -> anyhow::Result<Vec<String>> {
        let rows = sqlx::query(
            "SELECT DISTINCT to_id FROM relations \
             WHERE rel = ? AND from_type = ? AND from_id = ? AND status = 'active'",
        )
        .bind(REL_MENTIONS)
        .bind(from_type)
        .bind(from_id)
        .fetch_all(self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| r.get::<String, _>("to_id"))
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
    async fn record_is_idempotent() {
        let p = pool().await;
        let repo = MentionsRepository::new(&p);
        assert!(repo
            .record(
                "learning",
                "capture-decisions-a-1",
                "person",
                "person:djordje"
            )
            .await
            .unwrap());
        // same edge again → no new row.
        assert!(!repo
            .record(
                "learning",
                "capture-decisions-a-1",
                "person",
                "person:djordje"
            )
            .await
            .unwrap());
        let ents = repo
            .entities_for("learning", "capture-decisions-a-1")
            .await
            .unwrap();
        assert_eq!(ents, vec!["person:djordje".to_string()]);
    }

    #[tokio::test]
    async fn objects_mentioning_lists_sources() {
        let p = pool().await;
        let repo = MentionsRepository::new(&p);
        repo.record("learning", "obj-a", "person", "person:djordje")
            .await
            .unwrap();
        repo.record("learning", "obj-b", "person", "person:djordje")
            .await
            .unwrap();
        repo.record("learning", "obj-b", "project", "project:revesta")
            .await
            .unwrap();

        let mut djo = repo.objects_mentioning("person:djordje", 10).await.unwrap();
        djo.sort();
        assert_eq!(
            djo,
            vec![
                ("learning".to_string(), "obj-a".to_string()),
                ("learning".to_string(), "obj-b".to_string()),
            ]
        );
        let rev = repo
            .objects_mentioning("project:revesta", 10)
            .await
            .unwrap();
        assert_eq!(rev, vec![("learning".to_string(), "obj-b".to_string())]);
    }

    #[tokio::test]
    async fn clear_from_and_prefix_reconcile_edges() {
        let p = pool().await;
        let repo = MentionsRepository::new(&p);
        repo.record(
            "learning",
            "capture-decisions-a-1",
            "person",
            "person:djordje",
        )
        .await
        .unwrap();
        repo.record(
            "learning",
            "capture-decisions-b-2",
            "project",
            "project:revesta",
        )
        .await
        .unwrap();
        repo.record("learning", "capture-people-x-3", "person", "person:srdjan")
            .await
            .unwrap();

        // clear a single object's edges
        assert_eq!(
            repo.clear_from("learning", "capture-decisions-a-1")
                .await
                .unwrap(),
            1
        );
        assert!(repo
            .entities_for("learning", "capture-decisions-a-1")
            .await
            .unwrap()
            .is_empty());

        // clear all edges from one file by prefix (decisions), leave people untouched
        let removed = repo.clear_from_prefix("capture-decisions-").await.unwrap();
        assert_eq!(removed, 1, "only the remaining decisions edge");
        assert_eq!(
            repo.objects_mentioning("person:srdjan", 10)
                .await
                .unwrap()
                .len(),
            1,
            "people-file edge untouched by decisions prefix clear"
        );
    }

    #[tokio::test]
    async fn mention_counts_ranks_entities() {
        let p = pool().await;
        let repo = MentionsRepository::new(&p);
        repo.record("learning", "o1", "person", "person:djordje")
            .await
            .unwrap();
        repo.record("learning", "o2", "person", "person:djordje")
            .await
            .unwrap();
        repo.record("learning", "o3", "project", "project:revesta")
            .await
            .unwrap();
        let counts = repo.mention_counts().await.unwrap();
        assert_eq!(counts[0], ("person:djordje".to_string(), 2));
    }
}

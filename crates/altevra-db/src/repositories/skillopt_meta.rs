//! SkillOpt meta-fingerprint memory (PLAN-ALIVE §P3a) — `skillopt_meta`
//! (migration 037).
//!
//! Cross-run dedup for the skill-factory optimizer: every TRIED edit set is
//! recorded by an order-independent fingerprint (computed in
//! `altevra_skills::skill_edits::fingerprint_edits` — this crate stores only
//! the opaque hash). `was_tried` is consulted before any propose/queue step
//! so a failed edit set is never re-proposed (Hivemind `alreadyProposed`
//! semantics, SQLite-first instead of JSONL).
//!
//! `ops` carries short per-edit summaries (for feeding "what's been tried"
//! back into the proposer prompt) — NEVER full skill bodies.

use chrono::Utc;
use sqlx::{Row, SqlitePool};

use crate::util::ts_to_text;

/// The allowed outcome states (proposed → applied | reverted | rejected).
pub const SKILLOPT_OUTCOMES: &[&str] = &["proposed", "applied", "reverted", "rejected"];

#[derive(Debug, Clone)]
pub struct SkilloptMetaRow {
    pub id: String,
    pub skill_slug: String,
    /// sha256 hex, order-independent over the canonicalized edit set.
    pub fingerprint: String,
    /// JSON array of short per-edit summaries.
    pub ops: serde_json::Value,
    /// proposed | applied | reverted | rejected.
    pub outcome: String,
    pub tried_at: String,
}

pub struct SkilloptMetaRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> SkilloptMetaRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Record that an edit set was tried against a skill. Idempotent on
    /// `(skill_slug, fingerprint)` — re-recording updates `outcome`, `ops`
    /// and `tried_at` instead of duplicating.
    pub async fn record_tried(
        &self,
        skill_slug: &str,
        fingerprint: &str,
        ops: &serde_json::Value,
        outcome: &str,
    ) -> anyhow::Result<()> {
        if !SKILLOPT_OUTCOMES.contains(&outcome) {
            anyhow::bail!("skillopt outcome '{outcome}' is not one of {SKILLOPT_OUTCOMES:?}");
        }
        if skill_slug.is_empty() || fingerprint.is_empty() {
            anyhow::bail!("skill_slug and fingerprint must be non-empty");
        }
        let now = ts_to_text(&Utc::now());
        sqlx::query(
            "INSERT INTO skillopt_meta (id, skill_slug, fingerprint, ops, outcome, tried_at) \
             VALUES (?, ?, ?, ?, ?, ?) \
             ON CONFLICT(skill_slug, fingerprint) DO UPDATE SET \
               ops = excluded.ops, outcome = excluded.outcome, tried_at = excluded.tried_at",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(skill_slug)
        .bind(fingerprint)
        .bind(ops.to_string())
        .bind(outcome)
        .bind(&now)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Has this exact edit set (by fingerprint) already been tried for this
    /// skill? The proposer stops BEFORE queueing when this returns true.
    pub async fn was_tried(&self, skill_slug: &str, fingerprint: &str) -> anyhow::Result<bool> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM skillopt_meta WHERE skill_slug = ? AND fingerprint = ?",
        )
        .bind(skill_slug)
        .bind(fingerprint)
        .fetch_one(self.pool)
        .await?;
        Ok(count > 0)
    }

    /// All tried edit sets for a skill, newest first — the "what's been tried"
    /// feed for the proposer prompt (P3b/P3c).
    pub async fn list_for_skill(&self, skill_slug: &str) -> anyhow::Result<Vec<SkilloptMetaRow>> {
        let rows = sqlx::query(
            "SELECT id, skill_slug, fingerprint, ops, outcome, tried_at \
             FROM skillopt_meta WHERE skill_slug = ? ORDER BY tried_at DESC",
        )
        .bind(skill_slug)
        .fetch_all(self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| SkilloptMetaRow {
                id: r.get("id"),
                skill_slug: r.get("skill_slug"),
                fingerprint: r.get("fingerprint"),
                ops: serde_json::from_str(&r.get::<String, _>("ops"))
                    .unwrap_or(serde_json::json!([])),
                outcome: r.get("outcome"),
                tried_at: r.get("tried_at"),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::{create_pool, run_migrations};

    /// Per-test isolated DB in a TempDir (no shared state, no real ~/.altevra).
    async fn pool(dir: &tempfile::TempDir) -> SqlitePool {
        let db = dir.path().join("test.db");
        let p = create_pool(&format!("sqlite://{}?mode=rwc", db.display()))
            .await
            .unwrap();
        run_migrations(&p).await.unwrap();
        p
    }

    #[tokio::test]
    async fn was_tried_round_trip() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = pool(&dir).await;
        let repo = SkilloptMetaRepository::new(&p);

        let fp = "a".repeat(64);
        assert!(!repo.was_tried("my-skill", &fp).await.unwrap());

        repo.record_tried(
            "my-skill",
            &fp,
            &serde_json::json!(["append: new rule"]),
            "proposed",
        )
        .await
        .unwrap();

        assert!(repo.was_tried("my-skill", &fp).await.unwrap());
        // Different skill, same fingerprint → NOT tried (memory is per-skill).
        assert!(!repo.was_tried("other-skill", &fp).await.unwrap());
        // Same skill, different fingerprint → NOT tried.
        assert!(!repo.was_tried("my-skill", &"b".repeat(64)).await.unwrap());
    }

    #[tokio::test]
    async fn re_record_updates_outcome_without_duplicating() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = pool(&dir).await;
        let repo = SkilloptMetaRepository::new(&p);
        let fp = "c".repeat(64);

        repo.record_tried("s", &fp, &serde_json::json!([]), "proposed")
            .await
            .unwrap();
        repo.record_tried("s", &fp, &serde_json::json!(["replace 'x' -> 'y'"]), "applied")
            .await
            .unwrap();

        let rows = repo.list_for_skill("s").await.unwrap();
        assert_eq!(rows.len(), 1, "UNIQUE(skill_slug,fingerprint) must merge");
        assert_eq!(rows[0].outcome, "applied");
        assert_eq!(rows[0].ops, serde_json::json!(["replace 'x' -> 'y'"]));
    }

    #[tokio::test]
    async fn invalid_outcome_and_empty_keys_rejected() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = pool(&dir).await;
        let repo = SkilloptMetaRepository::new(&p);
        let fp = "d".repeat(64);
        assert!(repo
            .record_tried("s", &fp, &serde_json::json!([]), "maybe")
            .await
            .is_err());
        assert!(repo
            .record_tried("", &fp, &serde_json::json!([]), "proposed")
            .await
            .is_err());
        assert!(repo
            .record_tried("s", "", &serde_json::json!([]), "proposed")
            .await
            .is_err());
    }
}

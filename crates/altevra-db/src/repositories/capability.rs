//! Capability registry repositories (BUILD_TASKS T2.x, §5).
//!
//! Enforces the two load-bearing §5 guarantees at the persistence boundary:
//!  - **T7 honesty:** a `capability_record` may only be `supported` WITH an
//!    `evidence_ref`; without one it is rejected (no unproven native surface).
//!  - **dedup (T12):** a `skill_proposal` is keyed by `dedup_hash` — the same
//!    workflow proposes once; repeats increment `occurrences`, never duplicate.

use sqlx::{Row, SqlitePool};

use crate::util::ts_to_text;
use chrono::Utc;

/// Honest can/cannot/unverified ledger entry.
#[derive(Debug, Clone)]
pub struct CapabilityRecordRow {
    pub id: String,
    pub actor: String,
    pub capability_key: String,
    pub support: String, // supported|unsupported|unverified|fallback
    pub evidence_ref: Option<String>,
    pub verification_method: Option<String>,
}

pub struct CapabilityRecordsRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> CapabilityRecordsRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Upsert by (actor, capability_key). **T7:** rejects `supported` without an
    /// `evidence_ref` — honesty is enforced here, not trusted from the caller.
    pub async fn upsert(&self, row: &CapabilityRecordRow) -> anyhow::Result<()> {
        if row.support == "supported" && row.evidence_ref.as_deref().unwrap_or("").is_empty() {
            anyhow::bail!(
                "capability honesty (T7): '{}' cannot be 'supported' without an evidence_ref",
                row.capability_key
            );
        }
        let now = ts_to_text(&Utc::now());
        sqlx::query(
            "INSERT INTO capability_records \
             (id, actor, capability_key, support, evidence_ref, verification_method, verified_at, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(actor, capability_key) DO UPDATE SET \
               support=excluded.support, evidence_ref=excluded.evidence_ref, \
               verification_method=excluded.verification_method, verified_at=excluded.verified_at, \
               updated_at=excluded.updated_at",
        )
        .bind(&row.id)
        .bind(&row.actor)
        .bind(&row.capability_key)
        .bind(&row.support)
        .bind(row.evidence_ref.as_deref())
        .bind(row.verification_method.as_deref())
        .bind(if row.support == "supported" { Some(now.clone()) } else { None })
        .bind(&now)
        .bind(&now)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn get(&self, actor: &str, key: &str) -> anyhow::Result<Option<CapabilityRecordRow>> {
        let row = sqlx::query(
            "SELECT id, actor, capability_key, support, evidence_ref, verification_method \
             FROM capability_records WHERE actor = ? AND capability_key = ?",
        )
        .bind(actor)
        .bind(key)
        .fetch_optional(self.pool)
        .await?;
        Ok(row.map(|r| CapabilityRecordRow {
            id: r.get("id"),
            actor: r.get("actor"),
            capability_key: r.get("capability_key"),
            support: r.get("support"),
            evidence_ref: r.get("evidence_ref"),
            verification_method: r.get("verification_method"),
        }))
    }
}

pub struct SkillProposalsRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> SkillProposalsRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Propose a skill. **Dedup (T12):** if `dedup_hash` exists, increments
    /// `occurrences` and returns the existing id; never creates a second row.
    /// Returns `(id, is_new)`.
    pub async fn propose(
        &self,
        id: &str,
        dedup_hash: &str,
        proposed_slug: &str,
        proposed_body_json: &str,
    ) -> anyhow::Result<(String, bool)> {
        let existing = sqlx::query("SELECT id FROM skill_proposals WHERE dedup_hash = ?")
            .bind(dedup_hash)
            .fetch_optional(self.pool)
            .await?;
        if let Some(r) = existing {
            let existing_id: String = r.get("id");
            sqlx::query(
                "UPDATE skill_proposals SET occurrences = occurrences + 1, \
                 updated_at = ? WHERE dedup_hash = ?",
            )
            .bind(ts_to_text(&Utc::now()))
            .bind(dedup_hash)
            .execute(self.pool)
            .await?;
            return Ok((existing_id, false));
        }
        let now = ts_to_text(&Utc::now());
        sqlx::query(
            "INSERT INTO skill_proposals \
             (id, dedup_hash, proposed_slug, proposed_body, occurrences, status, created_at, updated_at) \
             VALUES (?, ?, ?, ?, 1, 'proposed', ?, ?)",
        )
        .bind(id)
        .bind(dedup_hash)
        .bind(proposed_slug)
        .bind(proposed_body_json)
        .bind(&now)
        .bind(&now)
        .execute(self.pool)
        .await?;
        Ok((id.to_string(), true))
    }

    pub async fn occurrences(&self, dedup_hash: &str) -> anyhow::Result<i64> {
        let row = sqlx::query("SELECT occurrences FROM skill_proposals WHERE dedup_hash = ?")
            .bind(dedup_hash)
            .fetch_optional(self.pool)
            .await?;
        Ok(row.map(|r| r.get::<i64, _>("occurrences")).unwrap_or(0))
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
    async fn supported_without_evidence_is_rejected() {
        let p = pool().await;
        let repo = CapabilityRecordsRepository::new(&p);
        let bad = CapabilityRecordRow {
            id: "c1".into(),
            actor: "claude-code".into(),
            capability_key: "hook.session_start".into(),
            support: "supported".into(),
            evidence_ref: None,
            verification_method: Some("declared".into()),
        };
        assert!(
            repo.upsert(&bad).await.is_err(),
            "T7: supported needs evidence"
        );

        let good = CapabilityRecordRow {
            evidence_ref: Some("verify_run:abc".into()),
            ..bad
        };
        assert!(repo.upsert(&good).await.is_ok());
        let got = repo
            .get("claude-code", "hook.session_start")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.support, "supported");
        assert_eq!(got.evidence_ref.as_deref(), Some("verify_run:abc"));
    }

    #[tokio::test]
    async fn skill_proposal_dedups_by_hash() {
        let p = pool().await;
        let repo = SkillProposalsRepository::new(&p);
        let (id1, new1) = repo
            .propose("sp1", "hash-xyz", "do-thing", "{}")
            .await
            .unwrap();
        assert!(new1);
        let (id2, new2) = repo
            .propose("sp2", "hash-xyz", "do-thing", "{}")
            .await
            .unwrap();
        assert!(!new2, "same workflow must not create a 2nd proposal");
        assert_eq!(id1, id2);
        assert_eq!(repo.occurrences("hash-xyz").await.unwrap(), 2);
    }
}

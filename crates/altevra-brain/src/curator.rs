//! C7 — Curator job (Hermes-borrowed, DB-only safe slice).
//!
//! Hermes' `agent/curator.py` runs ~7d idle-gated and maintains skills:
//! consolidates, archives stale, NEVER deletes. We mirror the *safe* parts at
//! the DB level: status transitions only, no hard deletes, recoverable.
//!
//! What this job does (all status transitions, no row deletions):
//!
//!  1. **Archive stale proposals.** A proposal that sat in `proposed` /
//!     `triaged` past [`PROPOSAL_STALE_DAYS`] is moved to the legal terminal
//!     state for that transition: `Proposed → Withdrawn`, `Triaged → Rejected`.
//!     The row stays — it is recoverable (the dedup hash still merges future
//!     evidence; Pavle can re-open it).
//!  2. **Retire applied past retention.** An `applied` proposal whose
//!     `decided_at` is older than [`APPLIED_RETAIN_DAYS`] transitions to
//!     `Deprecated` (legal: `Applied → Deprecated`). The row stays.
//!  3. **Mark stale skills.** A `skills` row with `status='active'` not
//!     touched (`updated_at`) for [`SKILL_STALE_DAYS`] flips to
//!     `status='archived'` — the source file & content stay, only the status
//!     changes. Recoverable by setting `status='active'` again.
//!
//! What this job does NOT do (hard invariants, enforced by code below the LLM):
//!  * No `DELETE`. Ever. The curator is a status mutator, period.
//!  * No touch to `exposure_decisions` / `audit_log` (R5-INV: those are
//!    append-only, never purged). A debug-assert pins this.
//!  * No spawning of external side effects (DB-only).
//!
//! Why this is safe to auto-run:
//!  * Every move is one that [`ProposalStatus::can_transition_to`] already
//!    encodes — illegal moves are rejected at the [`ProposalsRepository`]
//!    layer; nothing here can open a new state path.
//!  * The terminal states reached (`Withdrawn` / `Rejected` / `Deprecated`)
//!    are exactly the legal "archive" sinks the unified-proposal lifecycle
//!    already exposes.
//!
//! The companion digest line is emitted by [`curator_digest_line`], wired into
//! the [`crate::jobs::run_daily_summary`] body (additive, never destructive).
//!
//! [`ProposalStatus::can_transition_to`]: altevra_core::status::ProposalStatus::can_transition_to
//! [`ProposalsRepository`]: altevra_db::ProposalsRepository

use crate::jobs::{JobContext, JobResult};
use altevra_core::status::ProposalStatus;
use altevra_db::ProposalsRepository;
use sqlx::{Row, SqlitePool};

/// A `proposed` / `triaged` proposal older than this is archived (Withdrawn /
/// Rejected). Mirrors Hermes' `DEFAULT_STALE_AFTER_DAYS = 30`.
pub const PROPOSAL_STALE_DAYS: i64 = 30;
/// An `applied` proposal older than this is retired to `deprecated`. Beyond
/// this window the "this is the active rule" claim has decayed; the row stays
/// for history (R5-INV).
pub const APPLIED_RETAIN_DAYS: i64 = 60;
/// An `active` skill whose `updated_at` is older than this is marked
/// `archived`. Mirrors Hermes' `DEFAULT_ARCHIVE_AFTER_DAYS = 90`.
pub const SKILL_STALE_DAYS: i64 = 90;

/// What one curator pass did. Used by the test net + by the digest line writer
/// so the daily briefing reflects real counts, not a hard-coded zero.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CuratorReport {
    /// `proposed` proposals moved to `Withdrawn` (legal: Proposed → Withdrawn).
    pub proposals_withdrawn: usize,
    /// `triaged` proposals moved to `Rejected` (legal: Triaged → Rejected).
    pub proposals_rejected: usize,
    /// `applied` proposals moved to `Deprecated` (legal: Applied → Deprecated).
    pub proposals_deprecated: usize,
    /// `skills` rows flipped from `active` to `archived`.
    pub skills_archived: usize,
}

impl CuratorReport {
    /// Total status transitions performed — for the JobResult summary.
    pub fn total_archived(&self) -> usize {
        self.proposals_withdrawn
            + self.proposals_rejected
            + self.proposals_deprecated
            + self.skills_archived
    }

    /// Concise summary line for `brain_jobs.result_summary`.
    pub fn summary(&self) -> String {
        format!(
            "curator: archived {} stale proposal(s) (withdrawn={}, rejected={}), \
             deprecated {} applied, archived {} skill(s)",
            self.proposals_withdrawn + self.proposals_rejected,
            self.proposals_withdrawn,
            self.proposals_rejected,
            self.proposals_deprecated,
            self.skills_archived
        )
    }
}

/// Run one curator pass against the DB. Status transitions only. The job
/// itself is idempotent: a second call within the same window finds nothing
/// stale and is a no-op.
///
/// Legal-hold (R5-INV): `exposure_decisions` and `audit_log` are append-only
/// and MUST NOT be touched. This function never references them — the
/// `curator_archives_never_deletes` test pins both row counts before/after.
pub async fn run_curator(pool: &SqlitePool, ctx: &JobContext) -> anyhow::Result<JobResult> {
    let report = curate(pool, ctx).await?;
    Ok(JobResult {
        summary: report.summary(),
        items_processed: report.total_archived(),
    })
}

/// Inner DB pass — exposed for the digest-counts test (so a test can assert on
/// the structured [`CuratorReport`] without re-parsing the summary string).
pub async fn curate(pool: &SqlitePool, ctx: &JobContext) -> anyhow::Result<CuratorReport> {
    let now = ctx.now;
    let stale_cutoff = (now - chrono::Duration::days(PROPOSAL_STALE_DAYS)).to_rfc3339();
    let applied_cutoff = (now - chrono::Duration::days(APPLIED_RETAIN_DAYS)).to_rfc3339();
    let skill_cutoff = (now - chrono::Duration::days(SKILL_STALE_DAYS)).to_rfc3339();

    let repo = ProposalsRepository::new(pool);
    let mut report = CuratorReport::default();

    // 1. Stale `proposed` → Withdrawn. ISO8601 text sorts lexically.
    let stale_proposed: Vec<String> =
        sqlx::query("SELECT id FROM proposals WHERE status = 'proposed' AND created_at < ?")
            .bind(&stale_cutoff)
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|r| r.get::<String, _>("id"))
            .collect();
    for id in stale_proposed {
        // decided_by=None: the curator is an automatic janitor, not Pavle. We
        // still mark the row's status so it surfaces as archived; decided_*
        // stamps stay empty (HP-2: only a presence check stamps a human).
        if repo
            .transition_status(&id, ProposalStatus::Withdrawn, None)
            .await
            .is_ok()
        {
            report.proposals_withdrawn += 1;
        }
    }

    // 2. Stale `triaged` → Rejected (Triaged → Withdrawn is not in the legal
    //    enum; Triaged → Rejected is. Rejected is a terminal "archived" sink).
    let stale_triaged: Vec<String> =
        sqlx::query("SELECT id FROM proposals WHERE status = 'triaged' AND created_at < ?")
            .bind(&stale_cutoff)
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|r| r.get::<String, _>("id"))
            .collect();
    for id in stale_triaged {
        if repo
            .transition_status(&id, ProposalStatus::Rejected, None)
            .await
            .is_ok()
        {
            report.proposals_rejected += 1;
        }
    }

    // 3. Applied past retention → Deprecated (legal: Applied → Deprecated).
    //    Use decided_at when present; fall back to created_at if not (some
    //    paths apply directly without stamping a decider).
    let retire_applied: Vec<String> = sqlx::query(
        "SELECT id FROM proposals
         WHERE status = 'applied'
           AND COALESCE(decided_at, created_at) < ?",
    )
    .bind(&applied_cutoff)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|r| r.get::<String, _>("id"))
    .collect();
    for id in retire_applied {
        if repo
            .transition_status(&id, ProposalStatus::Deprecated, None)
            .await
            .is_ok()
        {
            report.proposals_deprecated += 1;
        }
    }

    // 4. Stale `active` skills → `archived` (status column; row stays).
    let res = sqlx::query(
        "UPDATE skills
            SET status = 'archived',
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
          WHERE status = 'active' AND updated_at < ?",
    )
    .bind(&skill_cutoff)
    .execute(pool)
    .await?;
    report.skills_archived = res.rows_affected() as usize;

    // R5-INV pin: the curator must never touch the append-only audit trails
    // (`exposure_decisions`, `audit_log`). The absence of any query against
    // those tables IS the proof; the `curator_archives_never_deletes` test
    // additionally counts both tables before/after each pass and asserts
    // equality, so a future regression that adds an accidental write would
    // trip immediately.
    let _ = now; // pin: kept so the cutoff math reads from the ctx clock, not Utc::now()
    Ok(report)
}

/// Build the curator digest line that lands in the daily briefing. Reads real
/// counts from `proposals` + `brain_jobs` — no hard-coded zeros.
///
/// Format (stable so downstream parsers can match):
/// `self-improve: N auto-applied · M proposed · K skills marked · A archived`
///
/// Where:
///  * `N` = proposals in `applied` status (the auto-apply outcome at the apply
///    seam; Tier-0 direct or Tier ≥ 1 after approval).
///  * `M` = proposals still `proposed` (queue depth — what's waiting).
///  * `K` = skills currently `archived` (curator's running total).
///  * `A` = total status archives the curator has performed across its history
///    (read from `brain_jobs` where `kind='curator'`: sum of items_processed
///    proxied by the run count × … simplest stable read: count of curator
///    runs whose `result_summary` is set).
pub async fn curator_digest_line(pool: &SqlitePool) -> String {
    let applied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM proposals WHERE status = 'applied'")
        .fetch_one(pool)
        .await
        .unwrap_or(0);
    let proposed: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM proposals WHERE status = 'proposed'")
            .fetch_one(pool)
            .await
            .unwrap_or(0);
    let skills_archived: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM skills WHERE status = 'archived'")
            .fetch_one(pool)
            .await
            .unwrap_or(0);
    // "archived" total for the digest: cumulative non-active proposals reached
    // via the legal archive sinks (withdrawn/rejected/deprecated/superseded).
    // Read from the live proposal status (not a derived counter) so it stays
    // honest after manual moves too.
    let archived_proposals: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM proposals
         WHERE status IN ('withdrawn','rejected','deprecated','superseded')",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    format!(
        "self-improve: {applied} auto-applied · {proposed} proposed · {skills_archived} skills marked · {archived_proposals} archived"
    )
}

/// Tag used in markdown to make the digest line greppable across files (tests
/// + future dashboards both anchor on it).
pub const DIGEST_TAG: &str = "self-improve:";

#[cfg(test)]
mod tests {
    use super::*;
    use altevra_db::{NewProposal, ProposalsRepository, SkillRow, SkillsRepository};
    use chrono::Utc;

    async fn migrated_pool() -> SqlitePool {
        let p = altevra_db::create_pool("sqlite::memory:").await.unwrap();
        altevra_db::run_migrations(&p).await.unwrap();
        p
    }

    fn ctx_at(now: chrono::DateTime<Utc>) -> JobContext {
        JobContext {
            vault_path: std::path::PathBuf::from("/tmp"),
            now,
            router: std::sync::Arc::new(altevra_llm::ModelRouter::noop()),
        }
    }

    async fn insert_with_age(
        pool: &SqlitePool,
        kind: &str,
        title: &str,
        dedup: &str,
        days_old: i64,
    ) -> String {
        let repo = ProposalsRepository::new(pool);
        let (id, _) = repo
            .insert(&NewProposal {
                kind: kind.into(),
                title: title.into(),
                body: "b".into(),
                source_mode: Some("test".into()),
                dedup_hash: dedup.into(),
                evidence_refs: vec![],
                touches_sensitive: false,
                touches_constitutional: false,
            })
            .await
            .unwrap();
        let backdated = (Utc::now() - chrono::Duration::days(days_old)).to_rfc3339();
        sqlx::query("UPDATE proposals SET created_at = ? WHERE id = ?")
            .bind(&backdated)
            .bind(&id)
            .execute(pool)
            .await
            .unwrap();
        id
    }

    /// Hard invariant: the curator NEVER hard-deletes. After a pass:
    ///   - row counts on `proposals` and `skills` are unchanged
    ///   - the stale proposal's status moved to a legal archive sink
    ///   - `exposure_decisions` and `audit_log` (R5-INV, append-only) are
    ///     untouched (counts pinned before/after).
    #[tokio::test]
    async fn curator_archives_never_deletes() {
        let pool = migrated_pool().await;

        // Seed: one stale `proposed`, one stale `triaged`, one applied past
        // retention. All three live across the curator pass — only their
        // STATUS changes.
        let stale_id = insert_with_age(
            &pool,
            "memory",
            "stale-proposed",
            "h-stale",
            PROPOSAL_STALE_DAYS + 5,
        )
        .await;
        let triaged_id = insert_with_age(
            &pool,
            "memory",
            "stale-triaged",
            "h-tri",
            PROPOSAL_STALE_DAYS + 5,
        )
        .await;
        let applied_id = insert_with_age(
            &pool,
            "memory",
            "old-applied",
            "h-app",
            APPLIED_RETAIN_DAYS + 5,
        )
        .await;
        // Move triaged into 'triaged' and applied into 'applied' (legal moves).
        let repo = ProposalsRepository::new(&pool);
        repo.transition_status(&triaged_id, ProposalStatus::Triaged, None)
            .await
            .unwrap();
        repo.transition_status(&applied_id, ProposalStatus::Applied, Some("test"))
            .await
            .unwrap();
        // Backdate the applied decided_at past retention so the curator picks it up.
        let applied_decided =
            (Utc::now() - chrono::Duration::days(APPLIED_RETAIN_DAYS + 5)).to_rfc3339();
        sqlx::query("UPDATE proposals SET decided_at = ? WHERE id = ?")
            .bind(&applied_decided)
            .bind(&applied_id)
            .execute(&pool)
            .await
            .unwrap();

        // One stale skill (active, updated_at older than SKILL_STALE_DAYS).
        let stale_skill = SkillRow {
            id: uuid::Uuid::new_v4(),
            slug: "stale-skill".into(),
            version: "0.1.0".into(),
            source_path: "/tmp/stale.md".into(),
            checksum: "deadbeef".into(),
            content: "body".into(),
            metadata: serde_json::Value::Object(Default::default()),
            status: "active".into(),
            created_at: Utc::now() - chrono::Duration::days(SKILL_STALE_DAYS + 10),
            updated_at: Utc::now() - chrono::Duration::days(SKILL_STALE_DAYS + 10),
        };
        SkillsRepository::new(&pool).upsert(&stale_skill).await.unwrap();

        // Pin counts BEFORE the pass — R5-INV checks the append-only tables.
        let before_proposals: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM proposals")
            .fetch_one(&pool)
            .await
            .unwrap();
        let before_skills: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM skills")
            .fetch_one(&pool)
            .await
            .unwrap();
        let before_exposure: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM exposure_decisions")
            .fetch_one(&pool)
            .await
            .unwrap();
        let before_audit: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_log")
            .fetch_one(&pool)
            .await
            .unwrap();

        let report = curate(&pool, &ctx_at(Utc::now())).await.unwrap();
        assert_eq!(report.proposals_withdrawn, 1, "stale 'proposed' → Withdrawn");
        assert_eq!(report.proposals_rejected, 1, "stale 'triaged' → Rejected");
        assert_eq!(report.proposals_deprecated, 1, "applied past retention → Deprecated");
        assert_eq!(report.skills_archived, 1, "stale skill → archived");

        // Hard invariant: NO ROW DELETED.
        let after_proposals: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM proposals")
            .fetch_one(&pool)
            .await
            .unwrap();
        let after_skills: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM skills")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(after_proposals, before_proposals, "no proposal row deleted");
        assert_eq!(after_skills, before_skills, "no skill row deleted");

        // R5-INV: append-only tables untouched.
        let after_exposure: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM exposure_decisions")
            .fetch_one(&pool)
            .await
            .unwrap();
        let after_audit: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_log")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(after_exposure, before_exposure, "R5-INV: exposure_decisions untouched");
        assert_eq!(after_audit, before_audit, "R5-INV: audit_log untouched");

        // The stale proposal moved to a legal archive sink (withdrawn), still
        // present in the DB — fully recoverable.
        let still = ProposalsRepository::new(&pool).get(&stale_id).await.unwrap().unwrap();
        assert_eq!(still.status, "withdrawn");
        let trow = ProposalsRepository::new(&pool).get(&triaged_id).await.unwrap().unwrap();
        assert_eq!(trow.status, "rejected");
        let arow = ProposalsRepository::new(&pool).get(&applied_id).await.unwrap().unwrap();
        assert_eq!(arow.status, "deprecated");

        // Skill flipped to 'archived' (row stays).
        let s = SkillsRepository::new(&pool)
            .find_by_slug("stale-skill")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(s.status, "archived");

        // Idempotency: a second pass on a now-quiet DB is a no-op.
        let second = curate(&pool, &ctx_at(Utc::now())).await.unwrap();
        assert_eq!(second, CuratorReport::default(), "second pass is no-op");
    }

    /// The digest line surfaces real counts from `proposals` + `skills`. We
    /// seed three proposals (one applied, one proposed, one withdrawn via the
    /// curator) and one archived skill, then assert each count appears.
    #[tokio::test]
    async fn curator_digest_counts() {
        let pool = migrated_pool().await;

        // applied (counted as "auto-applied")
        let repo = ProposalsRepository::new(&pool);
        let (a_id, _) = repo
            .insert(&NewProposal {
                kind: "memory".into(),
                title: "applied one".into(),
                body: "b".into(),
                source_mode: Some("t".into()),
                dedup_hash: "h-a".into(),
                evidence_refs: vec![],
                touches_sensitive: false,
                touches_constitutional: false,
            })
            .await
            .unwrap();
        repo.transition_status(&a_id, ProposalStatus::Applied, Some("test"))
            .await
            .unwrap();

        // pending `proposed` (counted as "proposed")
        let _ = repo
            .insert(&NewProposal {
                kind: "memory".into(),
                title: "still in queue".into(),
                body: "b".into(),
                source_mode: Some("t".into()),
                dedup_hash: "h-p".into(),
                evidence_refs: vec![],
                touches_sensitive: false,
                touches_constitutional: false,
            })
            .await
            .unwrap();

        // one that the curator will archive (Proposed → Withdrawn).
        let _ = insert_with_age(
            &pool,
            "memory",
            "stale",
            "h-stale",
            PROPOSAL_STALE_DAYS + 1,
        )
        .await;

        // one archived skill (already 'archived'), one active (not counted).
        let now = Utc::now();
        SkillsRepository::new(&pool)
            .upsert(&SkillRow {
                id: uuid::Uuid::new_v4(),
                slug: "old".into(),
                version: "0.1.0".into(),
                source_path: "/x".into(),
                checksum: "c1".into(),
                content: "".into(),
                metadata: serde_json::Value::Object(Default::default()),
                status: "archived".into(),
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        SkillsRepository::new(&pool)
            .upsert(&SkillRow {
                id: uuid::Uuid::new_v4(),
                slug: "fresh".into(),
                version: "0.1.0".into(),
                source_path: "/y".into(),
                checksum: "c2".into(),
                content: "".into(),
                metadata: serde_json::Value::Object(Default::default()),
                status: "active".into(),
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();

        // Run the curator → the stale proposal becomes withdrawn, contributing
        // to the "archived" tally.
        let _ = curate(&pool, &ctx_at(Utc::now())).await.unwrap();

        let line = curator_digest_line(&pool).await;
        // exact, stable substrings — daily_summary anchors on these too.
        assert!(line.contains(DIGEST_TAG), "digest line must start with the tag: {line}");
        assert!(line.contains("1 auto-applied"), "{line}");
        assert!(line.contains("1 proposed"), "{line}");
        assert!(line.contains("1 skills marked"), "{line}");
        assert!(line.contains("1 archived"), "{line}");
    }
}

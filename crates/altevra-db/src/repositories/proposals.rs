//! Unified proposals repository (migration 028, R10 §4.19#3).
//!
//! ONE table with a `kind` discriminator holds every memory/wiki/prompt/
//! category/skill/improvement proposal the resident modes emit (legacy
//! `skill_proposals` from 023 stays its own table). This is the persistence
//! seam where a resident run's [`ResidentOutput`] becomes durable proposal rows.
//!
//! Load-bearing invariants enforced HERE, not trusted from the caller:
//!  - **dedup (SI-13):** a proposal is keyed by `dedup_hash` — the same signal
//!    proposes once; repeats increment `evidence_count` + merge `evidence_refs`,
//!    never a 2nd row (mirrors [`SkillProposalsRepository::propose`]).
//!  - **SI-9 (tier re-derive):** `risk_tier` is computed by core's
//!    [`derive_risk_tier`] from `kind` (+ sensitivity/constitutional flags); any
//!    agent-asserted tier is IGNORED. The repo never writes a caller-supplied
//!    tier string.
//!  - **status transitions:** [`transition_status`] only permits moves that
//!    [`ProposalStatus::can_transition_to`] allows — `proposed → applied` is the
//!    Tier-0 direct path the enum already encodes; `Tier ≥ 1` auto-apply is
//!    rejected at apply time by the firewall (a later seam), never opened here.
//!
//! [`ResidentOutput`]: altevra_core::resident::ResidentOutput
//! [`derive_risk_tier`]: altevra_core::selfimprove::derive_risk_tier
//! [`SkillProposalsRepository::propose`]: crate::SkillProposalsRepository::propose

use altevra_core::resident::ResidentOutput;
use altevra_core::selfimprove::{derive_risk_tier, RiskTier};
use altevra_core::status::ProposalStatus;
use chrono::Utc;
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::util::ts_to_text;

/// A unified proposal row (migration 028 column shape).
#[derive(Debug, Clone)]
pub struct ProposalRow {
    pub id: String,
    pub kind: String,
    /// SI-9: derived by core, never asserted by the agent.
    pub risk_tier: String,
    pub status: String,
    pub title: String,
    pub body: String,
    pub source_mode: Option<String>,
    pub dedup_hash: String,
    pub evidence_count: i64,
    /// JSON array of evidence pointers (turn/session/run refs).
    pub evidence_refs: String,
    pub decided_by: Option<String>,
    pub decided_at: Option<String>,
    pub created_at: String,
}

/// What the caller asks to persist. The repo derives `risk_tier` itself (SI-9),
/// so this struct carries the *inputs* to the deriver, never a tier string.
#[derive(Debug, Clone)]
pub struct NewProposal {
    pub kind: String,
    pub title: String,
    pub body: String,
    pub source_mode: Option<String>,
    /// Dedup key — collisions merge into the existing row (never a 2nd row).
    pub dedup_hash: String,
    pub evidence_refs: Vec<String>,
    /// SI-9 deriver input: does this touch health/relationship/identity/credential
    /// data? (≥ Tier-1). The repo decides the tier; the agent cannot.
    pub touches_sensitive: bool,
    /// SI-9 deriver input: does this target a locked/constitutional surface?
    /// (always Tier-2).
    pub touches_constitutional: bool,
}

pub struct ProposalsRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> ProposalsRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Insert a proposal. **Dedup (SI-13):** if `dedup_hash` already exists,
    /// increments `evidence_count` and unions the `evidence_refs`, returning the
    /// existing id — never a second row. **SI-9:** `risk_tier` is re-derived from
    /// `kind` here; any caller-asserted tier is ignored. Returns `(id, is_new)`.
    pub async fn insert(&self, p: &NewProposal) -> anyhow::Result<(String, bool)> {
        // SI-9: tier is derived, never asserted. Re-derive on every call.
        let tier: RiskTier =
            derive_risk_tier(&p.kind, p.touches_sensitive, p.touches_constitutional);

        let existing = sqlx::query(
            "SELECT id, evidence_count, evidence_refs FROM proposals WHERE dedup_hash = ?",
        )
        .bind(&p.dedup_hash)
        .fetch_optional(self.pool)
        .await?;

        if let Some(r) = existing {
            // Collision: merge, never a 2nd row (SI-13).
            let existing_id: String = r.get("id");
            let prior_refs: String = r.get("evidence_refs");
            let merged_refs = merge_evidence_refs(&prior_refs, &p.evidence_refs);
            sqlx::query(
                "UPDATE proposals SET evidence_count = evidence_count + 1, \
                 evidence_refs = ? WHERE dedup_hash = ?",
            )
            .bind(serde_json::to_string(&merged_refs)?)
            .bind(&p.dedup_hash)
            .execute(self.pool)
            .await?;
            return Ok((existing_id, false));
        }

        let id = Uuid::new_v4().to_string();
        let now = ts_to_text(&Utc::now());
        let refs_json = serde_json::to_string(&p.evidence_refs)?;
        sqlx::query(
            "INSERT INTO proposals \
             (id, kind, risk_tier, status, title, body, source_mode, dedup_hash, \
              evidence_count, evidence_refs, created_at) \
             VALUES (?, ?, ?, 'proposed', ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&p.kind)
        .bind(tier.as_str())
        .bind(&p.title)
        .bind(&p.body)
        .bind(p.source_mode.as_deref())
        .bind(&p.dedup_hash)
        // SI-13: evidence_count is an OCCURRENCE counter (the signal proposed
        // once → 1; each dedup collision increments it), mirroring skill_proposals.
        .bind(1_i64)
        .bind(&refs_json)
        .bind(&now)
        .execute(self.pool)
        .await?;
        Ok((id, true))
    }

    pub async fn get(&self, id: &str) -> anyhow::Result<Option<ProposalRow>> {
        let row = sqlx::query(
            "SELECT id, kind, risk_tier, status, title, body, source_mode, dedup_hash, \
             evidence_count, evidence_refs, decided_by, decided_at, created_at \
             FROM proposals WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.pool)
        .await?;
        Ok(row.map(row_to_proposal))
    }

    /// List proposals, optionally filtered by status and/or kind. Newest first.
    pub async fn list(
        &self,
        status: Option<&str>,
        kind: Option<&str>,
    ) -> anyhow::Result<Vec<ProposalRow>> {
        let rows = match (status, kind) {
            (Some(s), Some(k)) => sqlx::query(
                "SELECT id, kind, risk_tier, status, title, body, source_mode, dedup_hash, \
                 evidence_count, evidence_refs, decided_by, decided_at, created_at \
                 FROM proposals WHERE status = ? AND kind = ? ORDER BY created_at DESC",
            )
            .bind(s)
            .bind(k)
            .fetch_all(self.pool)
            .await?,
            (Some(s), None) => sqlx::query(
                "SELECT id, kind, risk_tier, status, title, body, source_mode, dedup_hash, \
                 evidence_count, evidence_refs, decided_by, decided_at, created_at \
                 FROM proposals WHERE status = ? ORDER BY created_at DESC",
            )
            .bind(s)
            .fetch_all(self.pool)
            .await?,
            (None, Some(k)) => sqlx::query(
                "SELECT id, kind, risk_tier, status, title, body, source_mode, dedup_hash, \
                 evidence_count, evidence_refs, decided_by, decided_at, created_at \
                 FROM proposals WHERE kind = ? ORDER BY created_at DESC",
            )
            .bind(k)
            .fetch_all(self.pool)
            .await?,
            (None, None) => sqlx::query(
                "SELECT id, kind, risk_tier, status, title, body, source_mode, dedup_hash, \
                 evidence_count, evidence_refs, decided_by, decided_at, created_at \
                 FROM proposals ORDER BY created_at DESC",
            )
            .fetch_all(self.pool)
            .await?,
        };
        Ok(rows.into_iter().map(row_to_proposal).collect())
    }

    /// Transition a proposal's status. Respects
    /// [`ProposalStatus::can_transition_to`]: an illegal move (e.g. a Tier ≥ 1
    /// `proposed → applied` jump that bypasses approval, or any move the enum
    /// does not encode) is rejected. The only direct `proposed → applied` the
    /// enum permits is the Tier-0 path; Tier-1/2 auto-apply is firewalled at the
    /// apply seam (not opened here). `decided_by`/`decided_at` are stamped when
    /// the target is a terminal decision (approved/applied/rejected) and a
    /// decider is supplied (HP-2: core sets this only after a presence check).
    pub async fn transition_status(
        &self,
        id: &str,
        next: ProposalStatus,
        decided_by: Option<&str>,
    ) -> anyhow::Result<()> {
        let current_str: String = sqlx::query("SELECT status FROM proposals WHERE id = ?")
            .bind(id)
            .fetch_optional(self.pool)
            .await?
            .map(|r| r.get::<String, _>("status"))
            .ok_or_else(|| anyhow::anyhow!("proposal '{id}' not found"))?;
        let current: ProposalStatus = current_str.parse().expect("infallible parse");

        if !current.can_transition_to(&next) {
            anyhow::bail!(
                "illegal proposal transition {current} → {next} (id={id}): \
                 not permitted by ProposalStatus::can_transition_to"
            );
        }

        let stamps_decision = matches!(
            next,
            ProposalStatus::Approved | ProposalStatus::Applied | ProposalStatus::Rejected
        );
        if stamps_decision && decided_by.is_some() {
            let now = ts_to_text(&Utc::now());
            sqlx::query(
                "UPDATE proposals SET status = ?, decided_by = ?, decided_at = ? WHERE id = ?",
            )
            .bind(next.to_string())
            .bind(decided_by)
            .bind(&now)
            .bind(id)
            .execute(self.pool)
            .await?;
        } else {
            sqlx::query("UPDATE proposals SET status = ? WHERE id = ?")
                .bind(next.to_string())
                .bind(id)
                .execute(self.pool)
                .await?;
        }
        Ok(())
    }
}

/// Write each proposal in a resident run's output as a `proposals` row.
///
/// **SI-14 (zero-on-invalid):** the caller passes the run's terminal status; if
/// the output did NOT pass schema validation (anything other than `Completed`),
/// ZERO rows are written and `0` is returned — no partial proposals from a
/// failed run. The `brain_jobs` output_json is recorded separately and stays
/// as-is (this write is additive).
///
/// **SI-9 (tier re-derive):** every row's `risk_tier` is derived by
/// [`ProposalsRepository::insert`] from the proposal's `kind`; any
/// agent-supplied tier on the [`ResidentProposal`] is never read. The dedup key
/// is a deterministic hash of `(mode, kind, title)` so the same mode re-emitting
/// the same proposal merges rather than duplicating (SI-13).
///
/// [`ResidentProposal`]: altevra_core::resident::ResidentProposal
pub async fn write_resident_proposals(
    repo: &ProposalsRepository<'_>,
    mode: &str,
    status: altevra_core::resident::ResidentRunStatus,
    output: &ResidentOutput,
) -> anyhow::Result<usize> {
    use altevra_core::resident::ResidentRunStatus;

    // SI-14: only a schema-valid completed run may produce proposal rows. A
    // FailedSchema / AbortedBudget / Skipped run writes NOTHING.
    if status != ResidentRunStatus::Completed {
        return Ok(0);
    }

    let mut written = 0usize;
    for prop in &output.proposals {
        let dedup = dedup_hash_for(mode, &prop.kind, &prop.title);
        let np = NewProposal {
            kind: prop.kind.clone(),
            title: prop.title.clone(),
            body: prop.body.clone(),
            source_mode: Some(mode.to_string()),
            dedup_hash: dedup,
            evidence_refs: prop.evidence_refs.clone(),
            // The resident contract does not carry sensitivity/constitutional
            // flags on the proposal; the deriver fails safe by `kind` (unknown →
            // Tier-1). Modes that touch sensitive/locked surfaces are gated by
            // their sensitivity_ceiling upstream (SI-7) and re-tiered at apply.
            touches_sensitive: false,
            touches_constitutional: false,
        };
        let (_, _is_new) = repo.insert(&np).await?;
        written += 1;
    }
    Ok(written)
}

/// Deterministic dedup key for a resident proposal: a stable hash of
/// `(mode, kind, title)`. Same mode re-emitting the same proposal → same hash →
/// merge (SI-13), never a duplicate row.
fn dedup_hash_for(mode: &str, kind: &str, title: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    mode.hash(&mut h);
    0u8.hash(&mut h);
    kind.hash(&mut h);
    0u8.hash(&mut h);
    title.hash(&mut h);
    format!("res:{:016x}", h.finish())
}

/// Union two evidence-ref sets, preserving prior order then appending new refs.
fn merge_evidence_refs(prior_json: &str, incoming: &[String]) -> Vec<String> {
    let mut out: Vec<String> = serde_json::from_str(prior_json).unwrap_or_default();
    for r in incoming {
        if !out.iter().any(|e| e == r) {
            out.push(r.clone());
        }
    }
    out
}

fn row_to_proposal(r: sqlx::sqlite::SqliteRow) -> ProposalRow {
    ProposalRow {
        id: r.get("id"),
        kind: r.get("kind"),
        risk_tier: r.get("risk_tier"),
        status: r.get("status"),
        title: r.get("title"),
        body: r.get("body"),
        source_mode: r.get("source_mode"),
        dedup_hash: r.get("dedup_hash"),
        evidence_count: r.get("evidence_count"),
        evidence_refs: r.get("evidence_refs"),
        decided_by: r.get("decided_by"),
        decided_at: r.get("decided_at"),
        created_at: r.get("created_at"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::{create_pool, run_migrations};
    use altevra_core::resident::{ResidentProposal, ResidentRunStatus};

    async fn pool() -> SqlitePool {
        let p = create_pool("sqlite::memory:").await.unwrap();
        run_migrations(&p).await.unwrap();
        p
    }

    fn new_proposal(kind: &str, title: &str, dedup: &str) -> NewProposal {
        NewProposal {
            kind: kind.into(),
            title: title.into(),
            body: "body".into(),
            source_mode: Some("memory_curator".into()),
            dedup_hash: dedup.into(),
            evidence_refs: vec!["turn:1".into()],
            touches_sensitive: false,
            touches_constitutional: false,
        }
    }

    #[tokio::test]
    async fn proposal_roundtrip() {
        let p = pool().await;
        let repo = ProposalsRepository::new(&p);

        // insert → get → list
        let (id, is_new) = repo.insert(&new_proposal("memory", "t1", "h1")).await.unwrap();
        assert!(is_new);
        let got = repo.get(&id).await.unwrap().unwrap();
        assert_eq!(got.kind, "memory");
        assert_eq!(got.status, "proposed");
        // SI-9: "memory" derives to tier0 — the repo set it, not the caller.
        assert_eq!(got.risk_tier, "tier0");
        assert_eq!(got.evidence_count, 1);

        // list by kind + by status
        let all = repo.list(None, None).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(repo.list(Some("proposed"), None).await.unwrap().len(), 1);
        assert_eq!(repo.list(None, Some("memory")).await.unwrap().len(), 1);
        assert_eq!(repo.list(Some("applied"), None).await.unwrap().len(), 0);

        // dedup_hash collision → increment, never a 2nd row.
        let mut dup = new_proposal("memory", "t1", "h1");
        dup.evidence_refs = vec!["turn:2".into()]; // a new ref to merge
        let (id2, is_new2) = repo.insert(&dup).await.unwrap();
        assert!(!is_new2, "collision must not create a 2nd row");
        assert_eq!(id, id2);
        assert_eq!(repo.list(None, None).await.unwrap().len(), 1, "still one row");
        let merged = repo.get(&id).await.unwrap().unwrap();
        assert_eq!(merged.evidence_count, 2, "occurrences incremented");
        // evidence_refs unioned (turn:1 + turn:2).
        let refs: Vec<String> = serde_json::from_str(&merged.evidence_refs).unwrap();
        assert!(refs.contains(&"turn:1".to_string()) && refs.contains(&"turn:2".to_string()));
    }

    #[tokio::test]
    async fn si9_tier_is_rederived_not_asserted() {
        let p = pool().await;
        let repo = ProposalsRepository::new(&p);
        // "skill" → Tier1 regardless of any agent claim; "research_insight" → Tier0.
        let (skill_id, _) = repo.insert(&new_proposal("skill", "s", "hs")).await.unwrap();
        assert_eq!(repo.get(&skill_id).await.unwrap().unwrap().risk_tier, "tier1");
        // constitutional input forces tier2 even for an otherwise-tier0 kind.
        let mut np = new_proposal("memory", "c", "hc");
        np.touches_constitutional = true;
        let (cid, _) = repo.insert(&np).await.unwrap();
        assert_eq!(repo.get(&cid).await.unwrap().unwrap().risk_tier, "tier2");
    }

    #[tokio::test]
    async fn transition_respects_can_transition_to() {
        let p = pool().await;
        let repo = ProposalsRepository::new(&p);
        let (id, _) = repo.insert(&new_proposal("memory", "t", "h")).await.unwrap();

        // legal: proposed → triaged
        repo.transition_status(&id, ProposalStatus::Triaged, None)
            .await
            .unwrap();
        assert_eq!(repo.get(&id).await.unwrap().unwrap().status, "triaged");
        // illegal: triaged → applied (enum does not encode this; must reject).
        assert!(repo
            .transition_status(&id, ProposalStatus::Applied, Some("pavle"))
            .await
            .is_err());
        // legal: triaged → approved (stamps decided_by/at)
        repo.transition_status(&id, ProposalStatus::Approved, Some("pavle"))
            .await
            .unwrap();
        let row = repo.get(&id).await.unwrap().unwrap();
        assert_eq!(row.status, "approved");
        assert_eq!(row.decided_by.as_deref(), Some("pavle"));
        assert!(row.decided_at.is_some());
        // legal: approved → applied
        repo.transition_status(&id, ProposalStatus::Applied, Some("pavle"))
            .await
            .unwrap();
        assert_eq!(repo.get(&id).await.unwrap().unwrap().status, "applied");
    }

    #[tokio::test]
    async fn tier0_direct_proposed_to_applied_allowed_by_enum() {
        // The enum encodes the Tier-0 direct path (proposed → applied). The repo
        // must honor it; Tier ≥ 1 auto-apply is firewalled later, not here.
        let p = pool().await;
        let repo = ProposalsRepository::new(&p);
        let (id, _) = repo.insert(&new_proposal("research_insight", "t", "h")).await.unwrap();
        repo.transition_status(&id, ProposalStatus::Applied, Some("core"))
            .await
            .unwrap();
        assert_eq!(repo.get(&id).await.unwrap().unwrap().status, "applied");
    }

    #[tokio::test]
    async fn resident_output_writes_proposal_rows() {
        let p = pool().await;
        let repo = ProposalsRepository::new(&p);

        // a schema-valid completed run with 2 proposals → 2 rows.
        let output = ResidentOutput {
            proposals: vec![
                ResidentProposal {
                    kind: "memory".into(),
                    title: "learned X".into(),
                    body: "b1".into(),
                    evidence_refs: vec!["turn:1".into()],
                },
                ResidentProposal {
                    kind: "wiki".into(),
                    title: "page Y".into(),
                    body: "b2".into(),
                    evidence_refs: vec![],
                },
            ],
        };
        let n = write_resident_proposals(
            &repo,
            "memory_curator",
            ResidentRunStatus::Completed,
            &output,
        )
        .await
        .unwrap();
        assert_eq!(n, 2);
        assert_eq!(repo.list(None, None).await.unwrap().len(), 2);
        // source_mode + derived tier carried through.
        let rows = repo.list(None, Some("memory")).await.unwrap();
        assert_eq!(rows[0].source_mode.as_deref(), Some("memory_curator"));
        assert_eq!(rows[0].risk_tier, "tier0");

        // SI-14: a schema-invalid run (FailedSchema) writes ZERO rows even if the
        // output struct happens to carry proposals.
        let n2 = write_resident_proposals(
            &repo,
            "memory_curator",
            ResidentRunStatus::FailedSchema,
            &output,
        )
        .await
        .unwrap();
        assert_eq!(n2, 0, "SI-14: failed-schema run writes no proposals");
        assert_eq!(
            repo.list(None, None).await.unwrap().len(),
            2,
            "still only the 2 from the completed run"
        );
    }
}

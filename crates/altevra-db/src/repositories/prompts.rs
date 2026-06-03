//! Prompt registry repository (migration 028, §4.8) — the DB seam under the pure
//! [`prompt_registry`] core. The decisions (SI-8 plan, SI-2 lock, SI-10 gate,
//! drift) are computed by core from a STRUCTURED snapshot; this repo only loads
//! that snapshot and EXECUTES the resulting plan transactionally.
//!
//! Load-bearing invariants enforced HERE (the transaction the pure core can't run):
//!  - **SI-8 (one active per slug):** [`PromptsRepository::apply_mint`] runs the
//!    core [`MintPlan`] in ONE transaction — insert the new row, deactivate every
//!    prior active row for the slug, activate the new one — so there is never a
//!    window with two active rows. A proposed-only plan (SI-10 not-yet-passed)
//!    inserts INACTIVE and touches nothing else.
//!  - **SI-2 (constitutional lock):** [`PromptsRepository::mint`] asks core
//!    [`mint_plan`] first; a `locked` slug returns [`MintError::ConstitutionalLock`]
//!    and NO SQL runs. Aggressive mode does not bypass it.
//!  - **SI-10 (shadow-eval gate):** [`PromptsRepository::try_auto_activate`] loads
//!    the candidate's `prompt_eval_results` row and the slug snapshot, lets core
//!    decide, and only runs the activate transaction on [`AutoActivateDecision::Activate`].
//!
//! [`prompt_registry`]: altevra_core::prompt_registry
//! [`mint_plan`]: altevra_core::prompt_registry::mint_plan
//! [`MintPlan`]: altevra_core::prompt_registry::MintPlan
//! [`MintError::ConstitutionalLock`]: altevra_core::prompt_registry::MintError::ConstitutionalLock
//! [`AutoActivateDecision::Activate`]: altevra_core::prompt_registry::AutoActivateDecision::Activate

use altevra_core::prompt_registry::{
    mint_plan, try_auto_activate as core_try_auto_activate, AutoActivateDecision, MintPlan,
    PromptEval, PromptRecord,
};
use sqlx::{Row, SqlitePool};

pub struct PromptsRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> PromptsRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Load every row for one slug (active + inactive), the snapshot the pure core
    /// reasons over. Ordered by version for determinism.
    pub async fn snapshot_for(&self, name: &str) -> anyhow::Result<Vec<PromptRecord>> {
        let rows = sqlx::query(
            "SELECT name, version, layer, body, locked, active FROM prompts \
             WHERE name = ? ORDER BY version",
        )
        .bind(name)
        .fetch_all(self.pool)
        .await?;
        Ok(rows.into_iter().map(row_to_record).collect())
    }

    /// Load every prompt row across all slugs (for render snapshots + drift audits).
    pub async fn snapshot_all(&self) -> anyhow::Result<Vec<PromptRecord>> {
        let rows = sqlx::query(
            "SELECT name, version, layer, body, locked, active FROM prompts \
             ORDER BY name, version",
        )
        .fetch_all(self.pool)
        .await?;
        Ok(rows.into_iter().map(row_to_record).collect())
    }

    /// The single active row for a slug, if any (SI-8 → at most one).
    pub async fn active(&self, name: &str) -> anyhow::Result<Option<PromptRecord>> {
        let row = sqlx::query(
            "SELECT name, version, layer, body, locked, active FROM prompts \
             WHERE name = ? AND active = 1 ORDER BY version LIMIT 1",
        )
        .bind(name)
        .fetch_optional(self.pool)
        .await?;
        Ok(row.map(row_to_record))
    }

    /// Mint a new version for a slug and apply it. **SI-2:** core refuses a locked
    /// slug before any SQL runs. **SI-8:** when `activate_now` is true the activate
    /// transaction guarantees exactly one active row. When false the row is inserted
    /// proposed-only (inactive). Returns the applied [`MintPlan`].
    pub async fn mint(
        &self,
        name: &str,
        new_version: i64,
        layer: &str,
        body: &str,
        activate_now: bool,
    ) -> anyhow::Result<MintPlan> {
        let snapshot = self.snapshot_for(name).await?;
        // SI-2 + monotonic checks live in core; a locked slug errors out here with
        // no SQL having run.
        let plan = mint_plan(&snapshot, name, new_version, layer, body, activate_now)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        self.apply_mint(&plan).await?;
        Ok(plan)
    }

    /// Execute a core [`MintPlan`] in ONE transaction (SI-8). Insert the new row,
    /// then (only when `activate_now`) deactivate every prior active version and
    /// activate the new one — so there is never a moment with two active rows.
    pub async fn apply_mint(&self, plan: &MintPlan) -> anyhow::Result<()> {
        let locked_flag = 0_i64; // a minted row is never constitutional (SI-2).
        let active_flag: i64 = if plan.activate_now { 1 } else { 0 };

        let mut tx = self.pool.begin().await?;

        sqlx::query(
            "INSERT INTO prompts (name, version, layer, body, locked, active) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&plan.name)
        .bind(plan.new_version)
        .bind(&plan.layer)
        .bind(&plan.body)
        .bind(locked_flag)
        .bind(active_flag)
        .execute(&mut *tx)
        .await?;

        if plan.activate_now {
            // Deactivate every prior active version named in the plan (SI-8). We
            // deactivate by NOT-this-version rather than trusting the list alone,
            // so any straggler active row is also cleared inside the same tx —
            // belt and suspenders for the one-active invariant.
            sqlx::query("UPDATE prompts SET active = 0 WHERE name = ? AND version <> ?")
                .bind(&plan.name)
                .bind(plan.activate_version)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE prompts SET active = 1 WHERE name = ? AND version = ?")
                .bind(&plan.name)
                .bind(plan.activate_version)
                .execute(&mut *tx)
                .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// Record a shadow A/B eval result for a candidate version (SI-10 gate input).
    pub async fn record_eval(&self, eval: &PromptEval) -> anyhow::Result<()> {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO prompt_eval_results \
             (id, prompt_name, candidate_version, baseline_version, score_delta, passed) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&eval.prompt_name)
        .bind(eval.candidate_version)
        .bind(eval.baseline_version)
        .bind(eval.score_delta)
        .bind(if eval.passed { 1_i64 } else { 0 })
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// The latest shadow eval for a candidate version, if any.
    pub async fn latest_eval(
        &self,
        name: &str,
        candidate_version: i64,
    ) -> anyhow::Result<Option<PromptEval>> {
        // `rowid` is the implicit monotonic insert order (the table is not WITHOUT
        // ROWID), giving a deterministic "latest" even when two evals share a
        // millisecond `created_at` — a random TEXT id would tie-break nondeterministically.
        let row = sqlx::query(
            "SELECT prompt_name, candidate_version, baseline_version, score_delta, passed \
             FROM prompt_eval_results WHERE prompt_name = ? AND candidate_version = ? \
             ORDER BY rowid DESC LIMIT 1",
        )
        .bind(name)
        .bind(candidate_version)
        .fetch_optional(self.pool)
        .await?;
        Ok(row.map(|r| PromptEval {
            prompt_name: r.get("prompt_name"),
            candidate_version: r.get("candidate_version"),
            baseline_version: r.get("baseline_version"),
            score_delta: r.get("score_delta"),
            passed: r.get::<i64, _>("passed") != 0,
        }))
    }

    /// SI-10 self-modify gate — the method the orchestrator (C2) calls. Load the
    /// candidate's eval + the slug snapshot, let core decide, and AUTO-APPLY the
    /// activate transaction ONLY on [`AutoActivateDecision::Activate`]. Every other
    /// verdict (no eval → stays proposed; regression → auto-reject; locked → SI-2)
    /// runs no SQL. Returns the core decision for the caller to record/log.
    pub async fn try_auto_activate(
        &self,
        name: &str,
        candidate_version: i64,
    ) -> anyhow::Result<AutoActivateDecision> {
        let snapshot = self.snapshot_for(name).await?;
        let eval = self.latest_eval(name, candidate_version).await?;
        let decision = core_try_auto_activate(&snapshot, name, candidate_version, eval.as_ref());

        if decision.is_activate() {
            // Build the activate plan from the snapshot (deactivate-old-then-
            // activate-new, SI-8) and apply it. The candidate row already exists
            // (it was minted proposed-only), so this is a pure activation: flip
            // every prior active row off and this one on, in one transaction.
            let mut tx = self.pool.begin().await?;
            sqlx::query("UPDATE prompts SET active = 0 WHERE name = ? AND version <> ?")
                .bind(name)
                .bind(candidate_version)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE prompts SET active = 1 WHERE name = ? AND version = ?")
                .bind(name)
                .bind(candidate_version)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
        }
        Ok(decision)
    }
}

fn row_to_record(r: sqlx::sqlite::SqliteRow) -> PromptRecord {
    PromptRecord {
        name: r.get("name"),
        version: r.get("version"),
        layer: r.get("layer"),
        body: r.get("body"),
        locked: r.get::<i64, _>("locked") != 0,
        active: r.get::<i64, _>("active") != 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::{create_pool, run_migrations};
    use altevra_core::prompt_registry::assert_one_active_per_slug;

    async fn pool() -> SqlitePool {
        let p = create_pool("sqlite::memory:").await.unwrap();
        run_migrations(&p).await.unwrap();
        p
    }

    fn eval(name: &str, cand: i64, base: i64, delta: f64, passed: bool) -> PromptEval {
        PromptEval {
            prompt_name: name.into(),
            candidate_version: cand,
            baseline_version: base,
            score_delta: delta,
            passed,
        }
    }

    #[tokio::test]
    async fn one_active_per_slug_after_mint_chain() {
        let p = pool().await;
        let repo = PromptsRepository::new(&p);

        // Seed an active v1 for a resident mode.
        repo.mint("resident:observer", 1, "mode", "v1 body", true)
            .await
            .unwrap();
        // Mint + activate v2 → v1 must be deactivated in the same tx (SI-8).
        repo.mint("resident:observer", 2, "mode", "v2 body", true)
            .await
            .unwrap();
        // Mint + activate v3.
        repo.mint("resident:observer", 3, "mode", "v3 body", true)
            .await
            .unwrap();

        let snap = repo.snapshot_for("resident:observer").await.unwrap();
        assert_eq!(snap.len(), 3);
        // Exactly one active row, and it is v3.
        assert!(assert_one_active_per_slug(&snap).is_ok());
        let active = repo.active("resident:observer").await.unwrap().unwrap();
        assert_eq!(active.version, 3);
        assert_eq!(active.body, "v3 body");
    }

    #[tokio::test]
    async fn constitutional_locked_cannot_be_replaced_normally() {
        let p = pool().await;
        let repo = PromptsRepository::new(&p);

        // `safety` is seeded locked=1 (migration 028). The normal mint path refuses.
        let err = repo
            .mint("safety", 2, "safety", "tampered", true)
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("constitutional_lock"));
        assert!(msg.contains("Tier-2"));

        // The seeded safety row is untouched: still v1, still active, still locked.
        let snap = repo.snapshot_for("safety").await.unwrap();
        assert_eq!(snap.len(), 1, "no new row inserted");
        assert_eq!(snap[0].version, 1);
        assert!(snap[0].locked);
        assert!(snap[0].active);

        // try_auto_activate also refuses (SI-2) even though no eval exists.
        let d = repo.try_auto_activate("altevra_rules", 2).await.unwrap();
        assert_eq!(
            d,
            AutoActivateDecision::ConstitutionalLock {
                name: "altevra_rules".into()
            }
        );
    }

    #[tokio::test]
    async fn self_modify_requires_passing_shadow_eval() {
        let p = pool().await;
        let repo = PromptsRepository::new(&p);

        // Active v1 + a proposed-only (inactive) candidate v2 — minted WITHOUT
        // activation because no eval has passed yet.
        repo.mint("resident:insight", 1, "mode", "v1", true)
            .await
            .unwrap();
        repo.mint("resident:insight", 2, "mode", "v2 candidate", false)
            .await
            .unwrap();
        // v2 is inactive; v1 still active.
        assert_eq!(
            repo.active("resident:insight").await.unwrap().unwrap().version,
            1
        );

        // No eval yet → stays proposed (no auto-activate, no SQL).
        let d = repo.try_auto_activate("resident:insight", 2).await.unwrap();
        assert!(matches!(d, AutoActivateDecision::StayProposed { .. }));
        assert_eq!(
            repo.active("resident:insight").await.unwrap().unwrap().version,
            1,
            "v1 still active without a passing eval"
        );

        // A regression eval → auto-reject (still no activation).
        repo.record_eval(&eval("resident:insight", 2, 1, -0.3, false))
            .await
            .unwrap();
        let d = repo.try_auto_activate("resident:insight", 2).await.unwrap();
        assert!(matches!(d, AutoActivateDecision::AutoReject { .. }));
        assert_eq!(
            repo.active("resident:insight").await.unwrap().unwrap().version,
            1
        );

        // A later PASSING eval → auto-activate v2, deactivating v1 (SI-8).
        repo.record_eval(&eval("resident:insight", 2, 1, 0.5, true))
            .await
            .unwrap();
        let d = repo.try_auto_activate("resident:insight", 2).await.unwrap();
        assert_eq!(d, AutoActivateDecision::Activate);
        let snap = repo.snapshot_for("resident:insight").await.unwrap();
        assert!(assert_one_active_per_slug(&snap).is_ok());
        assert_eq!(
            repo.active("resident:insight").await.unwrap().unwrap().version,
            2,
            "candidate is active only after a passing shadow eval"
        );
    }
}

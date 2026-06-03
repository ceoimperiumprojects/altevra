//! SelfImproveOrchestrator (C2) — the 7-stage self-improve loop that is the FIRST
//! real caller of the runaway [`firewall_check`](altevra_core::selfimprove::firewall_check).
//!
//! ```text
//!   1 CAPTURE  open improvement_signals + observer insights (SI-6: resident-authored excluded)
//!   2 CLUSTER  cluster_open by cluster_key; a cluster needs MIN_EVIDENCE (SI-5)
//!   3 DETECT   actionable cluster → a `proposals` row; risk_tier RE-DERIVED (SI-9)
//!   4 GATE     firewall_check(limits, state, action) for EACH candidate  ← first caller
//!   5 APPLY    AGGRESSIVE: Tier0 auto-apply (category/wiki/insight/memory);
//!              skill → mark-for-render (C4 picks up); prompt (non-locked) + passing
//!              shadow eval → auto-activate (SI-10); persona/source_of_truth → review_item;
//!              Tier-2 / constitutional-locked → NEVER applied (asserted in CODE, not prompt)
//!   6 MONITOR  transition proposal statuses; PERSIST FirewallState deltas (SI-11/SI-12 accumulate)
//!   7 RETIRE   mark superseded/stale proposals
//! ```
//!
//! Every gate that decides what may auto-apply is pure Rust BELOW the LLM
//! ([`firewall_check`], [`derive_risk_tier`], [`try_auto_activate`]). The aggressive
//! autonomy mode changes only HOW MUCH auto-applies (Tier-0 + non-locked prompts that
//! pass a shadow eval); it can NOT remove a brake:
//!   * **Tier-2 / constitutional-locked NEVER auto-applies** — enforced by
//!     [`firewall_check`] (`ConstitutionalLock` / `RequiresReview` deny) AND
//!     re-asserted in [`apply_candidate`] via [`debug_assert`] before any write.
//!   * **Kill switch** — [`resident_disabled`] is checked at the VERY TOP of
//!     [`run_self_improve`]; a set flag skips the whole loop (`skipped_disabled`).
//!   * **SI-10 self-modify gate** — a prompt candidate only self-activates through
//!     [`PromptsRepository::try_auto_activate`], which runs no SQL unless a passing
//!     `prompt_eval_results` row exists.
//!
//! [`derive_risk_tier`]: altevra_core::selfimprove::derive_risk_tier
//! [`try_auto_activate`]: altevra_db::PromptsRepository::try_auto_activate

use altevra_core::observer::detect_patterns;
use altevra_core::selfimprove::{
    derive_risk_tier, firewall_check, FirewallLimits, FirewallState, FirewallVerdict, ProposedAction,
    RiskTier,
};
use altevra_core::status::ProposalStatus;
use altevra_db::{
    EventsRepository, FirewallStateRepository, ImprovementSignalsRepository, NewProposal,
    ProposalRow, ProposalsRepository, PromptsRepository, ReviewItemRow, TasksRepository,
};
use altevra_mcp::packet_build::compile_gated_packet;
use sqlx::SqlitePool;

use crate::jobs::{JobContext, JobResult};

/// A cluster needs at least this many open signals before it is actionable (SI-5).
/// One stray signal is not yet a pattern worth proposing on.
const MIN_EVIDENCE: usize = 2;

/// How far back STAGE 1 loads events for observer pattern detection.
const OBSERVER_WINDOW_DAYS: i64 = 14;

/// Token budget for the whole-base context packet compiled per actionable cluster
/// in STAGE 3 (a compact summary, not a full dump — the packet is for grounding the
/// proposal, the apply decision is the firewall's).
const SELFIMPROVE_PACKET_TOKENS: usize = 1500;

/// The orchestrator runs UNDER this mode name for budget/firewall-state purposes
/// (its `resident_budgets` row supplies the run-budget limit; migration 027 seeds it).
const ORCHESTRATOR_MODE: &str = "observer";

/// The kill-switch flag file under `~/.imperium/` (mirrors `SYMBIOSIS_DISABLED`).
const RESIDENT_DISABLED_FILE: &str = "RESIDENT_DISABLED";
/// The kill-switch env var (one-shot disable without touching the filesystem).
const RESIDENT_DISABLED_ENV: &str = "RESIDENT_DISABLED";

/// Kill switch (mirror of `SYMBIOSIS_DISABLED`): the loop is disabled when the
/// `RESIDENT_DISABLED` env var is set (non-empty) OR a `~/.imperium/RESIDENT_DISABLED`
/// flag file exists. Checked at the VERY TOP of [`run_self_improve`] — a tripped
/// switch skips the entire loop before any capture/gate/apply runs.
///
/// This is intentionally a LIVE check (env + file), never a DB row: a stored
/// kill-switch could be silently flipped off by the very loop it is meant to stop.
pub fn resident_disabled() -> bool {
    if std::env::var(RESIDENT_DISABLED_ENV)
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
    {
        return true;
    }
    if let Ok(home) = std::env::var("HOME") {
        if std::path::Path::new(&home)
            .join(".imperium")
            .join(RESIDENT_DISABLED_FILE)
            .exists()
        {
            return true;
        }
    }
    false
}

/// What the orchestrator decided to do with one candidate proposal in STAGE 5.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// Tier-0, firewall-allowed, low-risk kind → auto-applied (status → applied).
    AutoApplied,
    /// A `skill` candidate → marked for the C4 skill-factory render path (status
    /// → triaged); NOT rendered here.
    MarkedForRender,
    /// A non-locked `prompt` candidate that passed its shadow eval → self-activated
    /// (SI-10); status → applied.
    PromptActivated,
    /// A `prompt` candidate without a passing shadow eval → stays proposed (SI-10).
    PromptStaysProposed,
    /// A `persona` / `source_of_truth` candidate → a review_item was created; the
    /// proposal is parked at triaged (awaiting Pavle), NEVER auto-applied.
    RoutedToReview,
    /// The firewall denied auto-apply (Tier ≥ 1, constitutional lock, circuit open,
    /// budget, cap, cooldown, …) → stays proposed for human review.
    DeniedStaysProposed,
}

/// A structured summary of one orchestrator run (also the JobResult source).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelfImproveReport {
    /// True when the kill switch tripped and the loop skipped entirely.
    pub skipped_disabled: bool,
    pub signals_seen: usize,
    pub clusters_actionable: usize,
    pub proposals_created: usize,
    pub candidates_gated: usize,
    pub auto_applied: usize,
    pub marked_for_render: usize,
    pub prompts_activated: usize,
    pub routed_to_review: usize,
    pub denied: usize,
    pub retired: usize,
}

impl SelfImproveReport {
    fn one_line(&self) -> String {
        if self.skipped_disabled {
            return "self-improve: skipped_disabled (RESIDENT_DISABLED kill switch)".into();
        }
        format!(
            "self-improve: {} signal(s), {} actionable cluster(s), {} proposal(s) created, \
             {} gated → {} auto-applied, {} skill-for-render, {} prompt(s) activated, \
             {} to review, {} denied, {} retired",
            self.signals_seen,
            self.clusters_actionable,
            self.proposals_created,
            self.candidates_gated,
            self.auto_applied,
            self.marked_for_render,
            self.prompts_activated,
            self.routed_to_review,
            self.denied,
            self.retired,
        )
    }
}

// ---------------------------------------------------------------------------
// STAGE 4 input — the structured action the firewall reads (SI-15: fields only)
// ---------------------------------------------------------------------------

/// Build the [`ProposedAction`] the firewall gates from a candidate proposal row.
/// The tier is RE-DERIVED here from `kind` (+ sensitivity/constitutional flags),
/// IGNORING any tier already stored on the row (SI-9). `is_auto_apply` reflects the
/// aggressive intent: Tier-0 low-risk kinds and non-locked prompt self-modifies want
/// to auto-apply; everything else is recorded, not applied.
fn action_for(row: &ProposalRow, shadow_eval_passed: Option<bool>) -> ProposedAction {
    // SI-9: never trust row.risk_tier — re-derive from the structured kind. The
    // constitutional/sensitive inputs come from the kind itself.
    let touches_constitutional = is_constitutional_kind(&row.kind);
    let touches_sensitive = is_sensitive_kind(&row.kind);

    // A NON-locked prompt self-modify is the aggressive Tier-0 auto path GATED by the
    // firewall's SI-10 shadow-eval check (check #8) — exactly as the firewall models a
    // prompt change. (The prompt-registry `try_auto_activate` is the deeper SI-2/SI-10
    // backstop that actually runs the activate transaction.) A prompt targeting a
    // constitutional-locked layer stays constitutional → Tier-2, never auto-applies.
    let tier = if row.kind == "prompt" && !touches_constitutional {
        RiskTier::Tier0
    } else {
        derive_risk_tier(&row.kind, touches_sensitive, touches_constitutional)
    };

    // Aggressive mode: a Tier-0 candidate (a low-risk kind, or a non-locked prompt
    // self-modify) intends to auto-apply. A skill marks-for-render (recorded, not
    // auto-applied). Anything review-bound (persona/SoT/sensitive → Tier ≥ 1) is
    // recorded only — the firewall would deny its auto-apply anyway (RequiresReview).
    let is_auto_apply = matches!(tier, RiskTier::Tier0) && row.kind != "skill";

    ProposedAction {
        kind: row.kind.clone(),
        risk_tier: tier,
        is_auto_apply,
        // A prompt (or anything) targeting a constitutional-locked layer is caught
        // here; the prompt registry's SI-2 lock is the deeper backstop.
        targets_locked: touches_constitutional,
        // Real-time cooldown/dedup is upstream (the proposal dedup_hash merges
        // repeats); the firewall's per-window budget + circuit breaker do the
        // accumulation here.
        dedup_seen_within_cooldown: false,
        shadow_eval_passed,
    }
}

/// Extract plain FTS terms from a cluster key for the whole-base packet query.
/// A key like `session:claude-code:revesta` yields `["claude-code", "revesta"]`
/// (the `session` discriminator is dropped — it is not a content term). Total +
/// pure so an empty/odd key just yields an empty term set (→ an empty packet).
fn cluster_terms(cluster_key: &str) -> Vec<String> {
    cluster_key
        .split(':')
        .skip(1) // drop the leading discriminator ("session"/"turn"/…)
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_string())
        .collect()
}

/// Compact, non-leaking summary of a compiled whole-base packet for the proposal
/// body: the item count + the titles that grounded it (titles are already gated by
/// the ExposureGate inside [`compile_gated_packet`], so nothing over-ceiling appears).
fn summarize_packet(packet: &altevra_core::packet::ContextPacket) -> String {
    if packet.items.is_empty() {
        return "context: no related base objects".to_string();
    }
    let titles: Vec<String> = packet
        .items
        .iter()
        .take(5)
        .map(|i| format!("{} ({})", i.title, i.object_type))
        .collect();
    format!(
        "context: {} related base object(s), {} tokens — {}",
        packet.items.len(),
        packet.tokens_used,
        titles.join("; ")
    )
}

/// Kinds that touch a constitutional / locked surface → always Tier-2 (SI-2/SI-9).
fn is_constitutional_kind(kind: &str) -> bool {
    matches!(kind, "safety" | "altevra_rules" | "constitution")
}

/// Kinds that touch sensitive identity/relationship surfaces → at least review.
fn is_sensitive_kind(kind: &str) -> bool {
    matches!(kind, "persona" | "source_of_truth" | "person" | "relationship")
}

// ---------------------------------------------------------------------------
// The loop
// ---------------------------------------------------------------------------

/// Run the full 7-stage self-improve loop once as a brain job. Triggered real-time
/// (a hook may invoke this) and periodically as a backstop. Thin wrapper over
/// [`run_self_improve_report`] that adapts the structured report to a [`JobResult`].
pub async fn run_self_improve(pool: &SqlitePool, ctx: &JobContext) -> anyhow::Result<JobResult> {
    let report = run_self_improve_report(pool, ctx).await?;
    Ok(JobResult {
        summary: report.one_line(),
        items_processed: report.auto_applied
            + report.prompts_activated
            + report.marked_for_render,
    })
}

/// The 7-stage loop, returning the STRUCTURED [`SelfImproveReport`] (the job wrapper
/// [`run_self_improve`] turns this into a `JobResult`). Tests assert on the report.
pub async fn run_self_improve_report(
    pool: &SqlitePool,
    ctx: &JobContext,
) -> anyhow::Result<SelfImproveReport> {
    let mut report = SelfImproveReport::default();

    // KILL SWITCH — at the very top, before ANY capture/gate/apply.
    if resident_disabled() {
        report.skipped_disabled = true;
        return Ok(report);
    }

    let window_key = ctx.now.format("%Y-%m-%d").to_string();
    let fw = FirewallStateRepository::new(pool);
    let limits = fw.limits_for(ORCHESTRATOR_MODE).await?;
    // Load the ACCUMULATED counters; the kill switch is the live check above.
    let mut state = fw.load(&window_key).await?;

    let signals_repo = ImprovementSignalsRepository::new(pool);
    let proposals = ProposalsRepository::new(pool);

    // ---- STAGE 1 CAPTURE ----
    // Open signals. (SI-6 self-write exclusion is enforced at the PRODUCER, where a
    // resident-authored session never becomes a signal; we double-check the leaf
    // here so a stray resident-authored source_ref can never seed a proposal.)
    let all_signals = signals_repo.list_open().await?;
    report.signals_seen = all_signals.len();

    // Observer insights over recent events — keyless + LIVE; they become improvement
    // candidates alongside the clustered signals.
    let since = ctx.now - chrono::Duration::days(OBSERVER_WINDOW_DAYS);
    let events = EventsRepository::new(pool)
        .list_since(since, None, 2000)
        .await
        .unwrap_or_default();
    let insights = detect_patterns(&events, &[]);

    // ---- STAGE 2 CLUSTER ----  (SI-5: min_evidence)
    let clusters = signals_repo.cluster_open().await?;

    // ---- STAGE 3 DETECT ----  cluster → a `proposals` row (tier re-derived by the repo).
    for cluster in &clusters {
        // SI-5: a cluster needs MIN_EVIDENCE open signals before it is actionable.
        if cluster.signals.len() < MIN_EVIDENCE {
            continue;
        }
        // SI-6 double-check: skip a cluster whose signals were resident-authored.
        if cluster
            .signals
            .iter()
            .any(|s| altevra_db::is_resident_authored(&s.source_ref))
        {
            continue;
        }
        report.clusters_actionable += 1;

        let key = cluster.cluster_key.as_deref().unwrap_or("ungrouped");
        let evidence_refs: Vec<String> =
            cluster.signals.iter().map(|s| s.source_ref.clone()).collect();

        // The DETECT input is the WHOLE base, not a skill-only view: compile the
        // gated context packet over the cluster's terms (R12 retrieval — ExposureGate
        // strictly first, so sensitivity ceilings hold). The compiled context summary
        // is attached to the proposal body so a downstream applier/reviewer sees what
        // the proposal rests on. A failure to compile never aborts the loop.
        let terms = cluster_terms(key);
        let context_note = match compile_gated_packet(pool, &terms, SELFIMPROVE_PACKET_TOKENS).await
        {
            Ok(packet) => summarize_packet(&packet),
            Err(_) => "context: (packet compile unavailable)".to_string(),
        };

        // A clustered session-ingest pattern → a Tier-0 `improvement` proposal for
        // review/auto-apply. The repo re-derives the tier (SI-9); the dedup_hash keys
        // on the cluster so repeated runs MERGE rather than flood (SI-13).
        let np = NewProposal {
            kind: "improvement".into(),
            title: format!("Recurring pattern in {key}"),
            body: format!(
                "{} signal(s) clustered under `{key}` — review for a memory/learning/skill \
                 improvement.\n\n{context_note}",
                cluster.signals.len()
            ),
            source_mode: Some("self_improve".into()),
            dedup_hash: format!("selfimprove:cluster:{key}"),
            evidence_refs,
            touches_sensitive: false,
            touches_constitutional: false,
        };
        let (_, is_new) = proposals.insert(&np).await?;
        if is_new {
            report.proposals_created += 1;
        }
    }

    // Observer insights → improvement proposals too (deduped on the insight title).
    for ins in &insights {
        let np = NewProposal {
            kind: "improvement".into(),
            title: ins.title.clone(),
            body: ins.summary.clone(),
            source_mode: Some("observer".into()),
            dedup_hash: format!("selfimprove:insight:{}", ins.title),
            evidence_refs: ins
                .evidence
                .iter()
                .filter_map(|e| e.event_id.map(|id| format!("event:{id}")))
                .collect(),
            touches_sensitive: false,
            touches_constitutional: false,
        };
        let (_, is_new) = proposals.insert(&np).await?;
        if is_new {
            report.proposals_created += 1;
        }
    }

    // ---- STAGE 4 GATE + STAGE 5 APPLY ----  over EVERY open (proposed) proposal.
    let open = proposals.list(Some(ProposalStatus::Proposed.to_string().as_str()), None).await?;
    for row in &open {
        report.candidates_gated += 1;
        // STAGE 6 (partial): every gated candidate counts as a run against the
        // per-window budget (SI / SI-11 accumulation).
        state.runs_in_window = state.runs_in_window.saturating_add(1);

        match apply_candidate(pool, &proposals, row, &limits, &state).await? {
            ApplyOutcome::AutoApplied => {
                report.auto_applied += 1;
                state.auto_applies_in_window = state.auto_applies_in_window.saturating_add(1);
            }
            ApplyOutcome::PromptActivated => {
                report.prompts_activated += 1;
                state.auto_applies_in_window = state.auto_applies_in_window.saturating_add(1);
            }
            ApplyOutcome::MarkedForRender => report.marked_for_render += 1,
            ApplyOutcome::RoutedToReview => report.routed_to_review += 1,
            ApplyOutcome::PromptStaysProposed => {}
            ApplyOutcome::DeniedStaysProposed => report.denied += 1,
        }
    }

    // ---- STAGE 6 MONITOR ----  persist the accumulated firewall counters so the
    // circuit breaker + Tier-0 daily cap accumulate ACROSS runs (not reset to zero).
    fw.save(&window_key, &state).await?;

    // ---- STAGE 7 RETIRE ----  mark stale/superseded applied proposals deprecated.
    report.retired = retire_stale(&proposals).await?;

    Ok(report)
}

/// STAGE 4 + 5 for a single candidate proposal: gate via the firewall, then route
/// by kind/tier under aggressive autonomy. Returns the [`ApplyOutcome`].
///
/// **The firewall is the first gate — nothing applies that it denies.** Tier-2 /
/// constitutional candidates are denied by [`firewall_check`] (`ConstitutionalLock`
/// / `RequiresReview`); a [`debug_assert`] re-states the invariant in code right
/// before any apply so a future refactor can't accidentally open the path.
async fn apply_candidate(
    pool: &SqlitePool,
    proposals: &ProposalsRepository<'_>,
    row: &ProposalRow,
    limits: &FirewallLimits,
    state: &FirewallState,
) -> anyhow::Result<ApplyOutcome> {
    // A prompt candidate needs its shadow-eval verdict for the firewall's SI-10 gate.
    // (We load the latest eval for the active→candidate version pair.)
    let shadow_eval_passed = if row.kind == "prompt" {
        prompt_shadow_eval_passed(pool, row).await
    } else {
        None
    };

    let action = action_for(row, shadow_eval_passed);

    // STAGE 4 — the FIRST real firewall caller.
    let verdict = firewall_check(limits, state, &action);
    if let FirewallVerdict::Deny(_reason) = verdict {
        // Denied → stays proposed for human review. This is the path a Tier-2 /
        // constitutional / circuit-open / budget-exhausted candidate takes; it is
        // NEVER applied.
        return Ok(ApplyOutcome::DeniedStaysProposed);
    }

    // CODE-level re-assertion of the constitutional invariant (NOT a prompt): the
    // firewall must already have denied a Tier-2 / locked auto-apply; if it allowed
    // an auto-apply we are about to perform, it can only be Tier-0 (or a prompt that
    // will go through the SI-10 registry gate). This can not be flipped by note text.
    debug_assert!(
        !(action.is_auto_apply && action.risk_tier == RiskTier::Tier2),
        "firewall must never allow auto-apply of a Tier-2 action"
    );
    debug_assert!(
        !(action.is_auto_apply && action.targets_locked),
        "firewall must never allow auto-apply of a constitutional-locked target"
    );

    // STAGE 5 APPLY — aggressive routing by kind.
    match row.kind.as_str() {
        // A skill candidate is NOT rendered here — it is MARKED for the C4 skill-
        // factory render path (next workflow). Park it at `triaged` so C4 picks it up.
        "skill" => {
            proposals
                .transition_status(&row.id, ProposalStatus::Triaged, None)
                .await?;
            Ok(ApplyOutcome::MarkedForRender)
        }
        // A prompt self-modify (SI-10): the registry's `try_auto_activate` runs the
        // activate transaction ONLY when a passing shadow eval exists; a locked slug
        // (SI-2) or a missing/failing eval runs no SQL. We mark the proposal applied
        // ONLY when the prompt actually activated.
        "prompt" => apply_prompt(pool, proposals, row).await,
        // Persona / source-of-truth → a review_item (NOT auto). The proposal parks at
        // triaged awaiting Pavle; the firewall already denies its auto-apply (Tier-1),
        // so reaching here for these kinds means is_auto_apply was false — record for review.
        "persona" | "source_of_truth" => {
            create_review_item_for(pool, row).await?;
            proposals
                .transition_status(&row.id, ProposalStatus::Triaged, None)
                .await?;
            Ok(ApplyOutcome::RoutedToReview)
        }
        // Tier-0 low-risk kinds (category/wiki/insight/memory/improvement/...) →
        // auto-apply: proposed → applied (the enum's Tier-0 direct path). `decided_by`
        // is the orchestrator (a non-human applier; HP-2 is satisfied because Tier-0
        // needs no human presence — only Tier ≥ 1, which the firewall already denied).
        _ => {
            proposals
                .transition_status(&row.id, ProposalStatus::Applied, Some("self_improve"))
                .await?;
            Ok(ApplyOutcome::AutoApplied)
        }
    }
}

/// SI-10 self-modify: drive the prompt-registry gate. The slug is the proposal's
/// title prefixed `resident_mode:` by convention; the candidate version is the next
/// after the active row. The registry's [`PromptsRepository::try_auto_activate`]
/// activates ONLY on a passing shadow eval (and never a locked slug, SI-2).
async fn apply_prompt(
    pool: &SqlitePool,
    proposals: &ProposalsRepository<'_>,
    row: &ProposalRow,
) -> anyhow::Result<ApplyOutcome> {
    use altevra_core::prompt_registry::AutoActivateDecision;

    let prompts = PromptsRepository::new(pool);
    let slug = prompt_slug_for(row);
    // The candidate version = active + 1 (the proposed-only row was minted by an
    // earlier seam; here we only decide activation).
    let candidate_version = match prompts.active(&slug).await? {
        Some(active) => active.version + 1,
        None => 1,
    };
    let decision = prompts.try_auto_activate(&slug, candidate_version).await?;
    match decision {
        AutoActivateDecision::Activate => {
            proposals
                .transition_status(&row.id, ProposalStatus::Applied, Some("self_improve"))
                .await?;
            Ok(ApplyOutcome::PromptActivated)
        }
        // No passing eval / regression / locked → stays proposed (SI-10 / SI-2).
        _ => Ok(ApplyOutcome::PromptStaysProposed),
    }
}

/// Load the latest shadow-eval verdict for a prompt candidate (the active→next pair).
/// `None` when no eval has run → the firewall denies the prompt auto-apply (SI-10).
async fn prompt_shadow_eval_passed(pool: &SqlitePool, row: &ProposalRow) -> Option<bool> {
    let prompts = PromptsRepository::new(pool);
    let slug = prompt_slug_for(row);
    let candidate_version = match prompts.active(&slug).await.ok().flatten() {
        Some(active) => active.version + 1,
        None => 1,
    };
    prompts
        .latest_eval(&slug, candidate_version)
        .await
        .ok()
        .flatten()
        .map(|e| e.passed)
}

/// The prompt slug a `prompt` proposal targets. Convention: the proposal title IS
/// the slug (e.g. `resident_mode:observer`). Kept pure + total so a malformed title
/// just yields a slug that has no active row (→ no activation), never a panic.
fn prompt_slug_for(row: &ProposalRow) -> String {
    row.title.trim().to_string()
}

/// Create a review_item for a persona / source-of-truth candidate (NOT auto-applied).
async fn create_review_item_for(pool: &SqlitePool, row: &ProposalRow) -> anyhow::Result<()> {
    let item = ReviewItemRow {
        id: uuid::Uuid::new_v4(),
        project_id: None,
        kind: format!("selfimprove_{}", row.kind),
        title: row.title.clone(),
        body: Some(row.body.clone()),
        status: "pending".into(),
        created_at: chrono::Utc::now(),
        metadata: serde_json::json!({
            "proposal_id": row.id,
            "proposal_kind": row.kind,
            "source": "self_improve",
        }),
    };
    TasksRepository::new(pool).create_review_item(&item).await
}

/// STAGE 7 RETIRE — mark applied proposals that have been superseded by a newer
/// applied proposal under the same dedup family as deprecated. Conservative: only
/// `applied → deprecated`/`superseded`, which the enum permits, and only when a
/// strictly newer applied row exists for the same dedup_hash family. Returns count.
async fn retire_stale(_proposals: &ProposalsRepository<'_>) -> anyhow::Result<usize> {
    // The dedup-hash merge (SI-13) already collapses repeats into one row, so under
    // the current schema there is no "older duplicate applied row" to supersede —
    // retirement is a no-op until a later seam tracks success-metric decay
    // (`applied → deprecated`). Kept as the STAGE 7 hook so the loop shape is complete
    // and the next workflow has a place to land decay logic.
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use altevra_core::prompt_registry::PromptEval;
    use altevra_db::{create_pool, run_migrations};

    // `RESIDENT_DISABLED` is a process-global env var; cargo runs tests as parallel
    // threads in ONE process, so the kill-switch test's set/remove would race with
    // the other loop tests (they'd wrongly see the switch tripped mid-run). Serialize
    // every test that runs the loop through this lock. An ASYNC mutex (tokio) so the
    // guard may be held across the `.await` points without tripping
    // `clippy::await_holding_lock`.
    static LOOP_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    async fn migrated_pool() -> SqlitePool {
        let p = create_pool("sqlite::memory:").await.unwrap();
        run_migrations(&p).await.unwrap();
        p
    }

    fn ctx_for(now: chrono::DateTime<chrono::Utc>) -> JobContext {
        JobContext {
            vault_path: std::path::PathBuf::from("/nonexistent"),
            now,
            router: std::sync::Arc::new(altevra_llm::ModelRouter::noop()),
        }
    }

    /// Seed a `proposed` proposal of a given kind directly (bypassing the cluster
    /// stage) so the apply routing is tested in isolation.
    async fn seed_proposed(pool: &SqlitePool, kind: &str, title: &str, dedup: &str) -> String {
        let repo = ProposalsRepository::new(pool);
        let (id, _) = repo
            .insert(&NewProposal {
                kind: kind.into(),
                title: title.into(),
                body: "candidate body".into(),
                source_mode: Some("test".into()),
                dedup_hash: dedup.into(),
                evidence_refs: vec!["turn:1".into()],
                touches_sensitive: false,
                touches_constitutional: false,
            })
            .await
            .unwrap();
        id
    }

    async fn status_of(pool: &SqlitePool, id: &str) -> String {
        ProposalsRepository::new(pool)
            .get(id)
            .await
            .unwrap()
            .unwrap()
            .status
    }

    #[tokio::test]
    async fn self_improve_applies_tier0_and_skill_marks_for_render() {
        let _g = LOOP_LOCK.lock().await;
        let pool = migrated_pool().await;
        // A Tier-0 `category` candidate → auto-applies.
        let cat = seed_proposed(&pool, "category", "New category: hobby", "c:cat").await;
        // A `skill` candidate → marked for render (NOT applied).
        let skill = seed_proposed(&pool, "skill", "skill: auto-commit", "c:skill").await;
        // A `persona` candidate → review_item (NOT auto).
        let persona = seed_proposed(&pool, "persona", "persona shift", "c:persona").await;
        // A Tier-2 / constitutional candidate → NEVER applied.
        let locked = seed_proposed(&pool, "safety", "tamper safety prompt", "c:safety").await;

        let report = run_self_improve_report(&pool, &ctx_for(chrono::Utc::now()))
            .await
            .unwrap();
        assert!(!report.skipped_disabled);

        // Tier-0 category auto-applied.
        assert_eq!(status_of(&pool, &cat).await, "applied");
        assert_eq!(report.auto_applied, 1);
        // skill marked for render → triaged, not applied.
        assert_eq!(status_of(&pool, &skill).await, "triaged");
        assert_eq!(report.marked_for_render, 1);
        // persona → review_item created, proposal triaged (parked for Pavle).
        assert_eq!(status_of(&pool, &persona).await, "triaged");
        assert_eq!(report.routed_to_review, 1);
        let reviews = TasksRepository::new(&pool)
            .list_review_items(Some("pending"), 10)
            .await
            .unwrap();
        assert_eq!(reviews.len(), 1);
        assert!(reviews[0].kind.contains("persona"));
        // Tier-2 / constitutional NEVER applied — stays proposed (firewall-denied).
        assert_eq!(
            status_of(&pool, &locked).await,
            "proposed",
            "a constitutional candidate must never auto-apply"
        );
        assert!(report.denied >= 1);
    }

    #[tokio::test]
    async fn self_improve_self_prompt_needs_shadow_eval() {
        let _g = LOOP_LOCK.lock().await;
        let pool = migrated_pool().await;
        let prompts = PromptsRepository::new(&pool);
        // A non-locked resident-mode prompt with an active v1 and a proposed-only v2.
        prompts
            .mint("resident_mode:observer", 1, "mode", "v1", true)
            .await
            .unwrap();
        prompts
            .mint("resident_mode:observer", 2, "mode", "v2 candidate", false)
            .await
            .unwrap();

        // A `prompt` proposal whose TITLE is the slug.
        let prop = seed_proposed(&pool, "prompt", "resident_mode:observer", "c:prompt").await;

        // --- No passing eval → NOT auto-activated (stays proposed, v1 still active). ---
        let report1 = run_self_improve_report(&pool, &ctx_for(chrono::Utc::now()))
            .await
            .unwrap();
        assert_eq!(report1.prompts_activated, 0);
        assert_eq!(status_of(&pool, &prop).await, "proposed");
        assert_eq!(
            prompts.active("resident_mode:observer").await.unwrap().unwrap().version,
            1,
            "without a passing eval the candidate must not self-activate"
        );

        // --- Record a PASSING shadow eval → now it auto-activates. ---
        prompts
            .record_eval(&PromptEval {
                prompt_name: "resident_mode:observer".into(),
                candidate_version: 2,
                baseline_version: 1,
                score_delta: 0.4,
                passed: true,
            })
            .await
            .unwrap();
        let report2 = run_self_improve_report(&pool, &ctx_for(chrono::Utc::now()))
            .await
            .unwrap();
        assert_eq!(report2.prompts_activated, 1);
        assert_eq!(status_of(&pool, &prop).await, "applied");
        assert_eq!(
            prompts.active("resident_mode:observer").await.unwrap().unwrap().version,
            2,
            "a passing shadow eval lets the prompt self-modify (SI-10)"
        );
    }

    #[tokio::test]
    async fn self_improve_circuit_breaker() {
        let _g = LOOP_LOCK.lock().await;
        let pool = migrated_pool().await;
        let fw = FirewallStateRepository::new(&pool);
        let window = chrono::Utc::now().format("%Y-%m-%d").to_string();

        // Pre-load the firewall state into an OPEN circuit (consecutive_failures at
        // the default threshold of 5). Under this state proposing/applying pauses.
        fw.save(
            &window,
            &FirewallState {
                runs_in_window: 0,
                auto_applies_in_window: 0,
                consecutive_failures: 5, // == FirewallLimits::default().circuit_breaker_failures
                kill_switch: false,
            },
        )
        .await
        .unwrap();

        // A flood of Tier-0 candidates that WOULD auto-apply with a closed circuit.
        for i in 0..10 {
            seed_proposed(&pool, "category", &format!("cat {i}"), &format!("c:{i}")).await;
        }
        let report = run_self_improve_report(&pool, &ctx_for(chrono::Utc::now()))
            .await
            .unwrap();
        // Circuit open → every candidate is denied; NOTHING auto-applies.
        assert_eq!(report.auto_applied, 0, "open circuit pauses auto-apply");
        assert_eq!(report.denied, report.candidates_gated);

        // State accumulated across the run: runs_in_window advanced by the candidates
        // gated (it persisted, so the next run sees the higher water mark).
        let reloaded = fw.load(&window).await.unwrap();
        assert!(
            reloaded.runs_in_window >= 10,
            "run budget accumulates across runs (got {})",
            reloaded.runs_in_window
        );
        assert_eq!(reloaded.consecutive_failures, 5, "breaker state persisted");
    }

    #[tokio::test]
    async fn self_improve_kill_switch() {
        let _g = LOOP_LOCK.lock().await;
        let pool = migrated_pool().await;
        seed_proposed(&pool, "category", "should not apply", "c:k").await;

        // Trip the kill switch via the env var (mirrors SYMBIOSIS_DISABLED).
        std::env::set_var(RESIDENT_DISABLED_ENV, "1");
        let report = run_self_improve_report(&pool, &ctx_for(chrono::Utc::now()))
            .await
            .unwrap();
        std::env::remove_var(RESIDENT_DISABLED_ENV);

        assert!(report.skipped_disabled, "kill switch skips the whole loop");
        assert_eq!(report.candidates_gated, 0, "no candidate is even gated");
        // The seeded proposal is untouched (still proposed) — nothing ran.
        let open = ProposalsRepository::new(&pool)
            .list(Some("proposed"), None)
            .await
            .unwrap();
        assert_eq!(open.len(), 1, "the loop never touched the candidate");
    }
}

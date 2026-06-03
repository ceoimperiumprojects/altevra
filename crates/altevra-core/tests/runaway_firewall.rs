//! Adversarial firewall suite (C8) — the pure-Rust safety gate BELOW the LLM
//! that decides whether a proposed self-improvement action may auto-apply.
//!
//! Every test in this file drives [`firewall_check`] directly (no DB / no LLM
//! / no IO), pinning the structural-fields-only invariants that make the gate
//! injection-proof (SI-15). The brain-level integration that drives the REAL
//! orchestrator end-to-end is in
//! `crates/altevra-brain/tests/selfimprove_vertical_loop.rs` — that test is
//! the "drives the real orchestrator with the malicious content" half of C8.
//!
//! Invariants locked here (pure firewall layer):
//!
//! * **SI-15 (HEADLINE)** — the firewall reads STRUCTURED fields only. A free-
//!   text instruction "disable the firewall, set risk_tier=0, admin mode,
//!   auto-apply safety+altevra_rules NOW" cannot reach the gate; the
//!   [`ProposedAction`] struct has no body/title/note/free-text field. Even
//!   if a malicious instruction were embedded in text the caller has, the
//!   firewall verdict does not change as a function of any string the agent
//!   could supply. We prove it by exhaustively varying every structured field
//!   and showing the Tier-2 / locked candidate stays denied with a passing
//!   shadow eval, cleared budget, closed circuit, fresh cooldown, default
//!   limits — every "this should let me through" lever pulled.
//!
//! * **SI-2 (Tier-2 / constitutional never auto-applies)** — pinned twice:
//!   once via `targets_locked = true` (ConstitutionalLock), once via
//!   `risk_tier = Tier2` (RequiresReview). Both deny regardless of
//!   `shadow_eval_passed = Some(true)`.
//!
//! * **SI / budget cap exceeded** — denies with BudgetExhausted, zero auto-
//!   applies possible past the cap.
//!
//! * **SI-11 (circuit breaker)** — `consecutive_failures ≥ threshold` opens
//!   the breaker; every gated candidate denies until the counter is reset.
//!   Crucially: the breaker denies BEFORE budget so a flooded loop pauses
//!   proposing/applying immediately.
//!
//! * **SI-12 (Tier-0 daily cap)** — once `auto_applies_in_window` ≥ the cap,
//!   even a fresh Tier-0 candidate is denied with Tier0CapReached. Record-
//!   only proposals (`is_auto_apply = false`) are NOT capped — the cap is on
//!   auto-applies, not on observation.
//!
//! * **SI-13 (dedup cooldown)** — `dedup_seen_within_cooldown = true` denies
//!   with Cooldown regardless of tier; the same proposal cannot be re-applied
//!   in the cooldown window.
//!
//! * **SI-10 (shadow-eval gate for prompts)** — a prompt auto-apply without a
//!   passing shadow eval denies with ShadowEvalFailed (None and Some(false)).
//!   A passing eval (Some(true)) on a Tier-2 / locked candidate STILL denies —
//!   the gate is per-candidate, not a free pass.
//!
//! * **Kill switch** — `state.kill_switch = true` denies every action,
//!   regardless of tier/locked/shadow/cooldown/budget.
//!
//! * **Reason ordering** — the firewall reports the most-protective denial
//!   first: kill switch → constitutional lock → circuit → budget → cooldown
//!   → tier/cap/shadow. This is what makes "denied for the right reason"
//!   testable.

use altevra_core::selfimprove::{
    firewall_check, FirewallDenyReason, FirewallLimits, FirewallState, FirewallVerdict,
    ProposedAction, RiskTier,
};

// ---------------------------------------------------------------------------
// builders — keep the tests readable
// ---------------------------------------------------------------------------

fn lim() -> FirewallLimits {
    FirewallLimits::default()
}

/// A Tier-0 auto-apply candidate — the "happy path" base; tests toggle one
/// field at a time to prove each gate.
fn auto_tier0(kind: &str) -> ProposedAction {
    let mut a = ProposedAction::record(kind, RiskTier::Tier0);
    a.is_auto_apply = true;
    a
}

// ---------------------------------------------------------------------------
// SI / budget cap exceeded → aborts, ZERO auto-applies
// ---------------------------------------------------------------------------

#[test]
fn budget_cap_exceeded_denies_every_candidate() {
    // Pre-load the per-window run budget at exactly the cap. The next gated
    // candidate must deny with BudgetExhausted — no auto-apply may proceed.
    let st = FirewallState {
        runs_in_window: lim().max_runs_per_window, // == 100 by default
        ..Default::default()
    };

    // Tier-0 auto-apply candidate: would normally pass.
    let v = firewall_check(&lim(), &st, &auto_tier0("category"));
    assert_eq!(
        v,
        FirewallVerdict::Deny(FirewallDenyReason::BudgetExhausted),
        "budget cap must deny a Tier-0 auto-apply"
    );

    // A record-only proposal (not an auto-apply) ALSO denies under budget —
    // the run budget counts EVERY gated candidate, observation included
    // (matches `run_self_improve_report` which increments `runs_in_window`
    // for every gated candidate before routing).
    let v2 = firewall_check(&lim(), &st, &ProposedAction::record("memory", RiskTier::Tier0));
    assert_eq!(v2, FirewallVerdict::Deny(FirewallDenyReason::BudgetExhausted));

    // Flooding 10 more candidates under a saturated budget: still ZERO allows.
    let mut allows = 0;
    for i in 0..10 {
        let mut a = auto_tier0("category");
        a.dedup_seen_within_cooldown = false;
        if firewall_check(&lim(), &st, &a).is_allowed() {
            allows += 1;
        }
        let _ = i;
    }
    assert_eq!(allows, 0, "budget cap is a hard floor: zero allows past it");
}

// ---------------------------------------------------------------------------
// SI-11 — proposal flood → circuit breaker opens, state persists across runs
// ---------------------------------------------------------------------------

#[test]
fn circuit_breaker_opens_at_threshold_and_pauses_loop() {
    let limits = lim();
    // One failure UNDER threshold: still closed → an unrelated Tier-0 passes.
    let almost = FirewallState {
        consecutive_failures: limits.circuit_breaker_failures.saturating_sub(1),
        ..Default::default()
    };
    assert!(
        firewall_check(&limits, &almost, &auto_tier0("category")).is_allowed(),
        "breaker must NOT open BEFORE the threshold (off-by-one guard)"
    );

    // At threshold: breaker opens. Every gated candidate denies with
    // CircuitOpen — proposing/applying is paused at the gate.
    let open = FirewallState {
        consecutive_failures: limits.circuit_breaker_failures,
        ..Default::default()
    };
    assert_eq!(
        firewall_check(&limits, &open, &auto_tier0("category")),
        FirewallVerdict::Deny(FirewallDenyReason::CircuitOpen),
        "breaker at threshold must deny CircuitOpen"
    );
    // ABOVE threshold: still denied (no self-healing in the firewall — the
    // caller persists the counter and only an explicit reset closes it).
    let overshoot = FirewallState {
        consecutive_failures: limits.circuit_breaker_failures + 7,
        ..Default::default()
    };
    assert_eq!(
        firewall_check(&limits, &overshoot, &auto_tier0("memory")),
        FirewallVerdict::Deny(FirewallDenyReason::CircuitOpen),
    );

    // INVARIANT: the breaker is a STATE input, not a hidden internal counter
    // — the same `open` state across many calls keeps denying. That's exactly
    // what makes the breaker "persist across runs" once the caller writes it
    // back to `firewall_state` (see the brain-level test for the persistence
    // half).
    for _ in 0..20 {
        assert_eq!(
            firewall_check(&limits, &open, &auto_tier0("category")),
            FirewallVerdict::Deny(FirewallDenyReason::CircuitOpen),
            "open breaker is a hard floor across repeated gate calls"
        );
    }
}

// ---------------------------------------------------------------------------
// SI-12 — Tier-0 daily cap overflow → defers (auto-apply only; observation OK)
// ---------------------------------------------------------------------------

#[test]
fn tier0_daily_cap_overflow_defers_auto_apply_only() {
    let limits = lim();
    let at_cap = FirewallState {
        auto_applies_in_window: limits.max_auto_applies_per_window, // == 20 by default
        ..Default::default()
    };

    // Tier-0 AUTO-APPLY → denied (Tier0CapReached).
    assert_eq!(
        firewall_check(&limits, &at_cap, &auto_tier0("category")),
        FirewallVerdict::Deny(FirewallDenyReason::Tier0CapReached),
    );

    // RECORD-ONLY (observation) under the same cap → STILL ALLOWED. The cap
    // is on AUTO-APPLY, not on observing/proposing: the loop can keep
    // gathering signals past the daily auto-apply cap, it just can't act on
    // them. (SI-12 = "defers", not "aborts".)
    assert!(
        firewall_check(&limits, &at_cap, &ProposedAction::record("memory", RiskTier::Tier0))
            .is_allowed(),
        "Tier-0 cap defers AUTO-APPLY only — observation/proposal still flows"
    );
}

// ---------------------------------------------------------------------------
// SI-13 — rejected dedup within cooldown → suppressed
// ---------------------------------------------------------------------------

#[test]
fn dedup_within_cooldown_suppresses_any_tier() {
    let limits = lim();
    let st = FirewallState::default();

    // Tier-0 dup → cooldown.
    let mut t0 = auto_tier0("memory");
    t0.dedup_seen_within_cooldown = true;
    assert_eq!(
        firewall_check(&limits, &st, &t0),
        FirewallVerdict::Deny(FirewallDenyReason::Cooldown),
    );

    // Tier-1 dup (review-bound kind) → still cooldown (cooldown checks
    // before tier; the dup must not turn into a 2nd review either).
    let mut t1 = ProposedAction::record("skill", RiskTier::Tier1);
    t1.dedup_seen_within_cooldown = true;
    assert_eq!(
        firewall_check(&limits, &st, &t1),
        FirewallVerdict::Deny(FirewallDenyReason::Cooldown),
    );

    // A fresh (non-dup) submission of the same kind/tier IS allowed — only
    // the in-cooldown bit is what suppresses.
    let mut t0_fresh = auto_tier0("memory");
    t0_fresh.dedup_seen_within_cooldown = false;
    assert!(firewall_check(&limits, &st, &t0_fresh).is_allowed());
}

// ---------------------------------------------------------------------------
// SI-2 — Tier-2 / constitutional NEVER auto-applies (the C1-hardening case
// as an explicit adversarial scenario)
// ---------------------------------------------------------------------------

#[test]
fn tier2_constitutional_never_auto_applies_even_with_everything_else_green() {
    // Budget cleared, circuit closed, no cooldown, kill switch off, default
    // limits — every "this should let me through" lever pulled in favor of
    // the candidate. A Tier-2 / constitutional-locked candidate STILL denies.
    let limits = lim();
    let st = FirewallState::default();

    // (a) Targets a locked surface (the C1 case: `safety` / `altevra_rules`
    //     slug). Even a "harmless" Tier-0 record-only proposal aimed at a
    //     locked target denies with ConstitutionalLock.
    let mut locked = ProposedAction::record("prompt", RiskTier::Tier0);
    locked.targets_locked = true;
    locked.is_auto_apply = false; // even record-only
    locked.shadow_eval_passed = Some(true); // even a passing shadow eval
    assert_eq!(
        firewall_check(&limits, &st, &locked),
        FirewallVerdict::Deny(FirewallDenyReason::ConstitutionalLock),
        "a locked target is constitutionally off-limits — auto-apply OR record-only"
    );

    // (b) Tier-2 auto-apply (kind/tier alone, no locked surface) → denies
    //     with RequiresReview. A "passing" shadow eval cannot save it.
    let mut t2 = ProposedAction::record("safety", RiskTier::Tier2);
    t2.is_auto_apply = true;
    t2.shadow_eval_passed = Some(true);
    assert_eq!(
        firewall_check(&limits, &st, &t2),
        FirewallVerdict::Deny(FirewallDenyReason::RequiresReview),
        "Tier-2 must NEVER auto-apply, even with a passing shadow eval (SI-2)"
    );

    // (c) Tier-1 auto-apply (skill/prompt without lock) → also denies
    //     (RequiresReview). The aggressive mode does not open Tier-1 either.
    let mut t1_auto = ProposedAction::record("skill", RiskTier::Tier1);
    t1_auto.is_auto_apply = true;
    assert_eq!(
        firewall_check(&limits, &st, &t1_auto),
        FirewallVerdict::Deny(FirewallDenyReason::RequiresReview),
    );
}

// ---------------------------------------------------------------------------
// SI-10 — shadow-eval gate for prompts (the deeper backstop is the registry's
// try_auto_activate; this gate is the firewall layer)
// ---------------------------------------------------------------------------

#[test]
fn prompt_auto_apply_without_passing_shadow_eval_denies() {
    let limits = lim();
    let st = FirewallState::default();

    // No eval recorded → ShadowEvalFailed.
    let mut p = ProposedAction::record("prompt", RiskTier::Tier0);
    p.is_auto_apply = true;
    p.shadow_eval_passed = None;
    assert_eq!(
        firewall_check(&limits, &st, &p),
        FirewallVerdict::Deny(FirewallDenyReason::ShadowEvalFailed),
    );

    // Failing eval → ShadowEvalFailed.
    p.shadow_eval_passed = Some(false);
    assert_eq!(
        firewall_check(&limits, &st, &p),
        FirewallVerdict::Deny(FirewallDenyReason::ShadowEvalFailed),
    );

    // Passing eval AND Tier-0 AND not locked → ALLOWED (this is the only
    // path a prompt self-modifies; SI-10).
    p.shadow_eval_passed = Some(true);
    assert!(firewall_check(&limits, &st, &p).is_allowed());
}

// ---------------------------------------------------------------------------
// SI-15 (HEADLINE) — prompt-injection adversarial: vary EVERY structured field
// to maximally try to flip the verdict on a Tier-2 / locked candidate. The
// firewall is field-only; no string the caller could ever supply changes
// anything, because no string is read.
// ---------------------------------------------------------------------------

#[test]
fn si15_prompt_injection_cannot_flip_the_verdict_field_sweep() {
    let limits = lim();
    let st = FirewallState::default();

    // Adversarial intent: a malicious agent (or a malicious note ingested as
    // a signal) wants the firewall to allow auto-apply of a constitutional
    // change. The structured action it CAN supply has these fields and only
    // these fields. We sweep every cell of the cube and assert: when
    // targets_locked OR risk_tier == Tier2, the verdict is always Deny.
    let kinds = [
        // including obviously constitutional kinds AND fake "low-risk" kinds
        // a malicious agent might re-label the change to in order to game
        // the gate. The firewall doesn't trust the kind alone for tier:
        // tier is supplied by the caller (the orchestrator re-derives it
        // via core::derive_risk_tier — SI-9), and a locked surface denies
        // regardless of tier.
        "safety", "altevra_rules", "constitution", "memory", "category",
        "improvement", "research_insight", "wiki", "prompt", "skill",
    ];
    let shadow_results = [None, Some(false), Some(true)];
    let cooldown_flags = [false, true];
    let auto_apply_flags = [false, true];

    let mut probed = 0usize;
    let mut allowed = 0usize;
    for kind in &kinds {
        for shadow in &shadow_results {
            for &cooldown in &cooldown_flags {
                for &auto in &auto_apply_flags {
                    let mut adversarial = ProposedAction {
                        kind: (*kind).to_string(),
                        // The agent SAYS Tier-2 (or we say locked) — both must
                        // deny no matter what the rest of the cube does. We
                        // test both axes (locked & Tier2) below.
                        risk_tier: RiskTier::Tier2,
                        is_auto_apply: auto,
                        targets_locked: true, // the locked-target axis
                        dedup_seen_within_cooldown: cooldown,
                        shadow_eval_passed: *shadow,
                    };
                    let v = firewall_check(&limits, &st, &adversarial);
                    probed += 1;
                    if v.is_allowed() {
                        allowed += 1;
                    }

                    // And the other axis: NOT locked but Tier-2 risk.
                    adversarial.targets_locked = false;
                    let v2 = firewall_check(&limits, &st, &adversarial);
                    probed += 1;
                    if v2.is_allowed() {
                        // A Tier-2 record-only (is_auto_apply=false) IS
                        // allowed by the firewall (it's just an observation
                        // — Tier-2 means it won't auto-apply, not that it
                        // can't be recorded). That's the only legal "allow"
                        // in the cube on the Tier-2 axis; anything trying
                        // to auto-apply must deny.
                        assert!(
                            !adversarial.is_auto_apply,
                            "Tier-2 with is_auto_apply=true must NEVER pass"
                        );
                        allowed += 1;
                    } else if adversarial.is_auto_apply {
                        // is_auto_apply + Tier-2 → must deny RequiresReview
                        // (or Cooldown if dup-flagged; both are denies).
                        match v2 {
                            FirewallVerdict::Deny(FirewallDenyReason::RequiresReview)
                            | FirewallVerdict::Deny(FirewallDenyReason::Cooldown) => {}
                            other => panic!(
                                "Tier-2 auto-apply must deny RequiresReview or Cooldown, \
                                 got {other:?} for kind={kind} shadow={shadow:?} \
                                 cooldown={cooldown} auto={auto}"
                            ),
                        }
                    }
                }
            }
        }
    }
    // We swept the cube. The locked-target axis allows ZERO — that's the
    // SI-2 / SI-15 invariant for constitutional targets.
    let locked_axis_size = kinds.len() * shadow_results.len() * cooldown_flags.len() * auto_apply_flags.len();
    let tier2_record_only_allowed = kinds.len() * shadow_results.len(); // cooldown=false, auto=false × all kinds × all shadows
    // probed = 2× cube (locked axis + tier2-only axis).
    assert_eq!(probed, 2 * locked_axis_size, "field sweep coverage");
    assert!(
        allowed <= tier2_record_only_allowed,
        "only Tier-2 record-only observations may pass; locked axis must always deny \
         (allowed={allowed}, ceiling={tier2_record_only_allowed})"
    );
}

#[test]
fn si15_passing_shadow_eval_does_not_open_a_locked_or_tier2_target() {
    // Explicit pin: a "perfect" shadow eval (Some(true)) is the LARGEST
    // legitimate lever an agent can pull to get a prompt auto-applied. It
    // must NOT, by itself, override the constitutional lock OR Tier-2.
    let limits = lim();
    let st = FirewallState::default();

    // (a) Locked target with a "passing" eval.
    let mut a = ProposedAction::record("prompt", RiskTier::Tier0);
    a.is_auto_apply = true;
    a.targets_locked = true;
    a.shadow_eval_passed = Some(true);
    assert_eq!(
        firewall_check(&limits, &st, &a),
        FirewallVerdict::Deny(FirewallDenyReason::ConstitutionalLock),
        "passing shadow eval cannot override a constitutional lock"
    );

    // (b) Tier-2 auto-apply with a "passing" eval.
    let mut b = ProposedAction::record("safety", RiskTier::Tier2);
    b.is_auto_apply = true;
    b.shadow_eval_passed = Some(true);
    assert_eq!(
        firewall_check(&limits, &st, &b),
        FirewallVerdict::Deny(FirewallDenyReason::RequiresReview),
        "passing shadow eval cannot override Tier-2 auto-apply ban"
    );
}

// ---------------------------------------------------------------------------
// kill switch — `state.kill_switch = true` denies everything
// ---------------------------------------------------------------------------

#[test]
fn kill_switch_denies_every_candidate_regardless_of_fields() {
    let limits = lim();
    let killed = FirewallState {
        kill_switch: true,
        ..Default::default()
    };
    // Three candidates that would each normally pass the gate:
    let candidates = [
        auto_tier0("category"),
        ProposedAction::record("memory", RiskTier::Tier0),
        {
            let mut p = ProposedAction::record("prompt", RiskTier::Tier0);
            p.is_auto_apply = true;
            p.shadow_eval_passed = Some(true);
            p
        },
    ];
    for c in &candidates {
        assert_eq!(
            firewall_check(&limits, &killed, c),
            FirewallVerdict::Deny(FirewallDenyReason::KillSwitch),
            "kill switch must deny every candidate (kind={})",
            c.kind
        );
    }
}

// ---------------------------------------------------------------------------
// reason ordering — most-protective first (kill > lock > circuit > budget >
// cooldown > tier/cap/shadow). This is what makes "denied for the right
// reason" testable in higher-level callers (and in the brain integration
// test, which asserts on report.denied counts driven by these reasons).
// ---------------------------------------------------------------------------

#[test]
fn reason_ordering_most_protective_first() {
    let limits = lim();
    // Construct a state + action where MULTIPLE denial reasons apply at once;
    // the firewall must report the most-protective one (the earliest gate).

    // kill switch trumps a locked target trumps an open circuit trumps a
    // budget exhaustion trumps a cooldown trumps tier/cap/shadow.
    let st_killed_locked_open_budget_cooldown = FirewallState {
        runs_in_window: limits.max_runs_per_window,
        consecutive_failures: limits.circuit_breaker_failures,
        auto_applies_in_window: limits.max_auto_applies_per_window,
        kill_switch: true,
    };
    let mut everything = auto_tier0("prompt");
    everything.targets_locked = true;
    everything.dedup_seen_within_cooldown = true;
    everything.shadow_eval_passed = None;
    assert_eq!(
        firewall_check(&limits, &st_killed_locked_open_budget_cooldown, &everything),
        FirewallVerdict::Deny(FirewallDenyReason::KillSwitch),
        "kill switch must be reported first"
    );

    // Drop kill switch → ConstitutionalLock next.
    let st = FirewallState {
        kill_switch: false,
        ..st_killed_locked_open_budget_cooldown
    };
    assert_eq!(
        firewall_check(&limits, &st, &everything),
        FirewallVerdict::Deny(FirewallDenyReason::ConstitutionalLock),
    );

    // Drop the lock → CircuitOpen next.
    let mut a2 = everything.clone();
    a2.targets_locked = false;
    assert_eq!(
        firewall_check(&limits, &st, &a2),
        FirewallVerdict::Deny(FirewallDenyReason::CircuitOpen),
    );

    // Drop the circuit → BudgetExhausted next.
    let st_no_circuit = FirewallState {
        consecutive_failures: 0,
        ..st
    };
    assert_eq!(
        firewall_check(&limits, &st_no_circuit, &a2),
        FirewallVerdict::Deny(FirewallDenyReason::BudgetExhausted),
    );

    // Drop the budget → Cooldown next (cooldown is checked before tier).
    let st_no_budget = FirewallState {
        runs_in_window: 0,
        ..st_no_circuit
    };
    assert_eq!(
        firewall_check(&limits, &st_no_budget, &a2),
        FirewallVerdict::Deny(FirewallDenyReason::Cooldown),
    );

    // Drop the cooldown → Tier0CapReached next (still at cap from above).
    let mut a3 = a2.clone();
    a3.dedup_seen_within_cooldown = false;
    assert_eq!(
        firewall_check(&limits, &st_no_budget, &a3),
        FirewallVerdict::Deny(FirewallDenyReason::Tier0CapReached),
    );

    // Drop the Tier-0 cap → ShadowEvalFailed next (kind=prompt, no eval).
    let st_clean = FirewallState::default();
    assert_eq!(
        firewall_check(&limits, &st_clean, &a3),
        FirewallVerdict::Deny(FirewallDenyReason::ShadowEvalFailed),
    );

    // Drop the shadow-eval failure → finally ALLOW.
    let mut a4 = a3.clone();
    a4.shadow_eval_passed = Some(true);
    assert!(firewall_check(&limits, &st_clean, &a4).is_allowed());
}

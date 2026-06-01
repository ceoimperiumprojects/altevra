//! Runaway firewall (working draft §4.7) — the pure-Rust safety gate BELOW the
//! LLM. It decides whether a proposed self-improvement action may proceed. Every
//! decision is a function of STRUCTURED fields only; there is no text path, so no
//! prompt and no prompt injection can flip it (SI-15).
//!
//! Guarantees: budget cap (SI), circuit breaker (SI-11), Tier-0 auto-apply cap
//! (SI-12), dedup + cooldown (SI-13), shadow-eval gate for prompt changes
//! (SI-10), constitutional lock (SI-2: locked targets are never auto-changed and
//! Tier-1/2 never auto-apply), and a global kill switch.

use super::RiskTier;

/// A proposed action presented to the firewall. STRUCTURED ONLY — the firewall
/// never inspects free text, so injection in a title/body cannot change a verdict.
#[derive(Debug, Clone)]
pub struct ProposedAction {
    pub kind: String,
    pub risk_tier: RiskTier,
    /// The caller intends to AUTO-APPLY this (vs merely record a proposal).
    pub is_auto_apply: bool,
    /// Targets a constitutional-locked prompt (safety/altevra_rules) or SoT law.
    pub targets_locked: bool,
    /// The same dedup_hash was seen within the cooldown window (SI-13).
    pub dedup_seen_within_cooldown: bool,
    /// For prompt changes: did a shadow A/B eval pass the gate? `None` = not run.
    pub shadow_eval_passed: Option<bool>,
}

impl ProposedAction {
    /// A minimal record-only proposal (not an auto-apply) of the given tier/kind.
    pub fn record(kind: &str, risk_tier: RiskTier) -> Self {
        Self {
            kind: kind.to_string(),
            risk_tier,
            is_auto_apply: false,
            targets_locked: false,
            dedup_seen_within_cooldown: false,
            shadow_eval_passed: None,
        }
    }
}

/// Per-window limits (read from `resident_budgets`/config by the caller).
#[derive(Debug, Clone)]
pub struct FirewallLimits {
    pub max_runs_per_window: u32,
    pub max_auto_applies_per_window: u32,
    pub circuit_breaker_failures: u32,
}

impl Default for FirewallLimits {
    fn default() -> Self {
        Self {
            max_runs_per_window: 100,
            max_auto_applies_per_window: 20,
            circuit_breaker_failures: 5,
        }
    }
}

/// Mutable counters the caller persists across runs.
#[derive(Debug, Clone, Default)]
pub struct FirewallState {
    pub runs_in_window: u32,
    pub auto_applies_in_window: u32,
    pub consecutive_failures: u32,
    pub kill_switch: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirewallVerdict {
    Allow,
    Deny(FirewallDenyReason),
}

impl FirewallVerdict {
    pub fn is_allowed(&self) -> bool {
        matches!(self, FirewallVerdict::Allow)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirewallDenyReason {
    /// Global kill switch is engaged.
    KillSwitch,
    /// Targets a constitutional-locked prompt — never auto-changed (SI-2).
    ConstitutionalLock,
    /// A Tier-1/Tier-2 action attempted to auto-apply (SI-2: review required).
    RequiresReview,
    /// Circuit breaker open after too many consecutive failures (SI-11).
    CircuitOpen,
    /// Per-window run budget exhausted.
    BudgetExhausted,
    /// Tier-0 auto-apply cap reached this window (SI-12).
    Tier0CapReached,
    /// Same proposal seen within the cooldown window (SI-13).
    Cooldown,
    /// A prompt change without a passing shadow eval (SI-10).
    ShadowEvalFailed,
}

impl FirewallDenyReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            FirewallDenyReason::KillSwitch => "kill_switch",
            FirewallDenyReason::ConstitutionalLock => "constitutional_lock",
            FirewallDenyReason::RequiresReview => "requires_review",
            FirewallDenyReason::CircuitOpen => "circuit_open",
            FirewallDenyReason::BudgetExhausted => "budget_exhausted",
            FirewallDenyReason::Tier0CapReached => "tier0_cap_reached",
            FirewallDenyReason::Cooldown => "cooldown",
            FirewallDenyReason::ShadowEvalFailed => "shadow_eval_failed",
        }
    }
}

/// The single firewall decision. Checks run most-protective first so a blocked
/// action reports the strongest reason. Pure: no IO, no clock, no text parsing.
pub fn firewall_check(
    limits: &FirewallLimits,
    state: &FirewallState,
    action: &ProposedAction,
) -> FirewallVerdict {
    use FirewallDenyReason as D;

    // 1. global kill switch.
    if state.kill_switch {
        return FirewallVerdict::Deny(D::KillSwitch);
    }
    // 2. constitutional lock (SI-2) — a locked target is NEVER auto-changed,
    //    regardless of tier, approval, or anything in the proposal text.
    if action.targets_locked {
        return FirewallVerdict::Deny(D::ConstitutionalLock);
    }
    // 3. circuit breaker (SI-11).
    if state.consecutive_failures >= limits.circuit_breaker_failures {
        return FirewallVerdict::Deny(D::CircuitOpen);
    }
    // 4. per-window run budget.
    if state.runs_in_window >= limits.max_runs_per_window {
        return FirewallVerdict::Deny(D::BudgetExhausted);
    }
    // 5. dedup + cooldown (SI-13).
    if action.dedup_seen_within_cooldown {
        return FirewallVerdict::Deny(D::Cooldown);
    }

    // Auto-apply-only checks.
    if action.is_auto_apply {
        // 6. SI-2: only Tier-0 may auto-apply.
        if !action.risk_tier.auto_appliable() {
            return FirewallVerdict::Deny(D::RequiresReview);
        }
        // 7. Tier-0 cap (SI-12).
        if state.auto_applies_in_window >= limits.max_auto_applies_per_window {
            return FirewallVerdict::Deny(D::Tier0CapReached);
        }
        // 8. prompt changes require a passing shadow eval (SI-10).
        if action.kind == "prompt" && action.shadow_eval_passed != Some(true) {
            return FirewallVerdict::Deny(D::ShadowEvalFailed);
        }
    }

    FirewallVerdict::Allow
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lim() -> FirewallLimits {
        FirewallLimits::default()
    }

    #[test]
    fn record_only_low_risk_is_allowed() {
        let v = firewall_check(
            &lim(),
            &FirewallState::default(),
            &ProposedAction::record("research_insight", RiskTier::Tier0),
        );
        assert!(v.is_allowed());
    }

    #[test]
    fn tier1_tier2_cannot_auto_apply() {
        // SI-2: only Tier-0 auto-applies.
        for tier in [RiskTier::Tier1, RiskTier::Tier2] {
            let mut a = ProposedAction::record("memory", tier);
            a.is_auto_apply = true;
            assert_eq!(
                firewall_check(&lim(), &FirewallState::default(), &a),
                FirewallVerdict::Deny(FirewallDenyReason::RequiresReview)
            );
        }
    }

    #[test]
    fn locked_target_is_constitutional_denied() {
        // Even a Tier-0 record-only action that targets a locked prompt is denied.
        let mut a = ProposedAction::record("prompt", RiskTier::Tier0);
        a.targets_locked = true;
        assert_eq!(
            firewall_check(&lim(), &FirewallState::default(), &a),
            FirewallVerdict::Deny(FirewallDenyReason::ConstitutionalLock)
        );
    }

    #[test]
    fn prompt_injection_cannot_change_the_gate() {
        // SI-15: the firewall reads STRUCTURED fields only. An action whose kind
        // is a Tier-2 prompt edit cannot be auto-applied no matter what — there is
        // no text the caller could supply to flip the verdict (the struct has no
        // free-text field the gate consults).
        let mut a = ProposedAction::record("prompt", RiskTier::Tier2);
        a.is_auto_apply = true;
        a.shadow_eval_passed = Some(true); // even a "passing" eval can't save Tier-2
        let v = firewall_check(&lim(), &FirewallState::default(), &a);
        assert!(!v.is_allowed());
        assert_eq!(v, FirewallVerdict::Deny(FirewallDenyReason::RequiresReview));
    }

    #[test]
    fn circuit_breaker_opens_after_failures() {
        let st = FirewallState {
            consecutive_failures: 5,
            ..Default::default()
        };
        assert_eq!(
            firewall_check(
                &lim(),
                &st,
                &ProposedAction::record("memory", RiskTier::Tier0)
            ),
            FirewallVerdict::Deny(FirewallDenyReason::CircuitOpen)
        );
    }

    #[test]
    fn budget_and_cap_and_cooldown_and_kill() {
        // budget
        let mut st = FirewallState {
            runs_in_window: 100,
            ..Default::default()
        };
        assert_eq!(
            firewall_check(
                &lim(),
                &st,
                &ProposedAction::record("memory", RiskTier::Tier0)
            ),
            FirewallVerdict::Deny(FirewallDenyReason::BudgetExhausted)
        );
        // tier-0 cap (auto-apply)
        st = FirewallState {
            auto_applies_in_window: 20,
            ..Default::default()
        };
        let mut auto = ProposedAction::record("memory", RiskTier::Tier0);
        auto.is_auto_apply = true;
        assert_eq!(
            firewall_check(&lim(), &st, &auto),
            FirewallVerdict::Deny(FirewallDenyReason::Tier0CapReached)
        );
        // cooldown
        let mut dup = ProposedAction::record("memory", RiskTier::Tier0);
        dup.dedup_seen_within_cooldown = true;
        assert_eq!(
            firewall_check(&lim(), &FirewallState::default(), &dup),
            FirewallVerdict::Deny(FirewallDenyReason::Cooldown)
        );
        // kill switch
        let killed = FirewallState {
            kill_switch: true,
            ..Default::default()
        };
        assert_eq!(
            firewall_check(
                &lim(),
                &killed,
                &ProposedAction::record("memory", RiskTier::Tier0)
            ),
            FirewallVerdict::Deny(FirewallDenyReason::KillSwitch)
        );
    }

    #[test]
    fn prompt_auto_apply_needs_passing_shadow_eval() {
        // SI-10: a Tier-0 prompt change without a passing shadow eval is denied.
        let mut a = ProposedAction::record("prompt", RiskTier::Tier0);
        a.is_auto_apply = true;
        a.shadow_eval_passed = None;
        assert_eq!(
            firewall_check(&lim(), &FirewallState::default(), &a),
            FirewallVerdict::Deny(FirewallDenyReason::ShadowEvalFailed)
        );
        a.shadow_eval_passed = Some(false);
        assert!(!firewall_check(&lim(), &FirewallState::default(), &a).is_allowed());
        a.shadow_eval_passed = Some(true);
        assert!(firewall_check(&lim(), &FirewallState::default(), &a).is_allowed());
    }
}

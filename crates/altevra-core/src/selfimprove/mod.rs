//! Self-improvement risk model + runaway firewall (working draft §4, R10).
//!
//! The self-improve loop (capture → cluster → detect → gate → apply → monitor →
//! retire) is LLM-driven, but the gates that decide what may auto-apply live HERE
//! in pure Rust, BELOW the LLM. No prompt — and no prompt injection — can change
//! them (SI-2/SI-15): a proposal is a struct of fields; the firewall reads only
//! those fields, never free text.

pub mod firewall;

pub use firewall::{
    firewall_check, FirewallDenyReason, FirewallLimits, FirewallState, FirewallVerdict,
    ProposedAction,
};

/// Risk tier of a proposed self-improvement (SI-9). Derived, never asserted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskTier {
    /// Auto-appliable, low-risk (research insight, low-risk wiki, new category,
    /// non-sensitive memory).
    Tier0,
    /// Review-required (skill/prompt/source-of-truth/person/relationship, or any
    /// sensitive write).
    Tier1,
    /// Never auto-apply; touches constitutional/locked targets — heavy review.
    Tier2,
}

impl RiskTier {
    pub fn as_str(&self) -> &'static str {
        match self {
            RiskTier::Tier0 => "tier0",
            RiskTier::Tier1 => "tier1",
            RiskTier::Tier2 => "tier2",
        }
    }

    /// Write-authority (§4.6): only Tier-0 may auto-apply; Tier-1/2 require a
    /// human-presence review (SI-2: Tier-2 can NEVER auto-apply).
    pub fn auto_appliable(&self) -> bool {
        matches!(self, RiskTier::Tier0)
    }
}

/// SI-9 risk-tier deriver — a PURE function (same inputs → same tier).
///
/// * `touches_constitutional` — targets a locked prompt (safety/altevra_rules) or
///   source-of-truth law → always Tier-2.
/// * `touches_sensitive` — health/relationship/identity/credential data → at
///   least Tier-1 (review).
pub fn derive_risk_tier(
    kind: &str,
    touches_sensitive: bool,
    touches_constitutional: bool,
) -> RiskTier {
    if touches_constitutional {
        return RiskTier::Tier2;
    }
    if touches_sensitive {
        return RiskTier::Tier1;
    }
    match kind {
        "skill" | "prompt" | "source_of_truth" | "person" | "relationship" => RiskTier::Tier1,
        "research_insight" | "wiki" | "category" | "memory" | "improvement" => RiskTier::Tier0,
        // Unknown kind → fail-safe to review.
        _ => RiskTier::Tier1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constitutional_target_is_always_tier2() {
        assert_eq!(derive_risk_tier("memory", false, true), RiskTier::Tier2);
        assert_eq!(
            derive_risk_tier("research_insight", false, true),
            RiskTier::Tier2
        );
        assert!(!RiskTier::Tier2.auto_appliable());
    }

    #[test]
    fn sensitive_write_requires_review() {
        assert_eq!(derive_risk_tier("memory", true, false), RiskTier::Tier1);
        assert!(!RiskTier::Tier1.auto_appliable());
    }

    #[test]
    fn low_risk_kinds_are_tier0_auto() {
        assert_eq!(
            derive_risk_tier("research_insight", false, false),
            RiskTier::Tier0
        );
        assert_eq!(derive_risk_tier("category", false, false), RiskTier::Tier0);
        assert!(RiskTier::Tier0.auto_appliable());
    }

    #[test]
    fn skill_and_prompt_are_review() {
        assert_eq!(derive_risk_tier("skill", false, false), RiskTier::Tier1);
        assert_eq!(derive_risk_tier("prompt", false, false), RiskTier::Tier1);
    }

    #[test]
    fn unknown_kind_fails_safe_to_review() {
        assert_eq!(derive_risk_tier("mystery", false, false), RiskTier::Tier1);
    }
}

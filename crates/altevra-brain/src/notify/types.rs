//! Notification contract (P4) — the Hivemind Source/Rule/Delivery shape
//! ported to Altevra (docs/research/hivemind/05-proactive-goals-dashboard.md).
//!
//! Invariants:
//!   * `user_visible_only` is **TRUE BY DEFAULT** — an item reaches the
//!     agent-injected channel only after an EXPLICIT opt-out
//!     ([`NotifyItem::allow_agent_channel`]). Guards against prompt-injection
//!     and against sensitive personal insight leaking into model context.
//!   * Rules are cadence-gated: one notification per item per cadence window
//!     (atomic O_EXCL claim files, see [`super::delivery`]).
//!   * High-precision-or-silent: sources return nothing over noise.

use chrono::{DateTime, Utc};

/// Rule ids (stable strings — used in claim filenames + audit rows).
pub const RULE_DECISION_STALENESS: &str = "decision_staleness";
pub const RULE_RELATIONSHIP_CADENCE: &str = "relationship_cadence";
pub const RULE_RESUME_BRIEF: &str = "resume_brief";
pub const RULE_OPEN_PROPOSALS: &str = "open_proposals";

/// A candidate notification produced by a source.
#[derive(Debug, Clone)]
pub struct NotifyItem {
    /// Which rule produced this (one of the `RULE_*` constants).
    pub rule: String,
    /// One-line headline (kept short — briefing bullet).
    pub title: String,
    /// 0–3 lines of detail.
    pub body: String,
    /// The `domain_policies.domain_key` governing this item's content
    /// (business/project/relationship/...). Drives the per-item
    /// `obsidian_mirror` consult in the delivery layer.
    pub domain_key: String,
    /// Identity of the underlying fact — same fact within a cadence window
    /// dedups; a changed fact re-fires.
    pub dedup_key: String,
    /// TRUE BY DEFAULT — deliver to the user-facing channels only, never to
    /// agent-injected context. Opt-out is explicit via
    /// [`Self::allow_agent_channel`].
    pub user_visible_only: bool,
}

impl NotifyItem {
    pub fn new(
        rule: &str,
        domain_key: &str,
        title: impl Into<String>,
        body: impl Into<String>,
        dedup_key: impl Into<String>,
    ) -> Self {
        Self {
            rule: rule.to_string(),
            title: title.into(),
            body: body.into(),
            domain_key: domain_key.to_string(),
            dedup_key: dedup_key.into(),
            // The load-bearing default: unflagged rules NEVER reach an
            // agent-injected channel.
            user_visible_only: true,
        }
    }

    /// EXPLICIT opt-out of user-visible-only — the only way an item becomes
    /// eligible for the agent context channel.
    pub fn allow_agent_channel(mut self) -> Self {
        self.user_visible_only = false;
        self
    }
}

/// Per-rule minimum re-fire interval (cadence gate). The claim filename
/// embeds the cadence bucket so a second fire inside the window collides on
/// the same O_EXCL claim file and is suppressed.
pub fn min_interval_hours(rule: &str) -> i64 {
    match rule {
        // A contact-gap nudge per person at most once a week — the brain
        // doesn't nag every session (Hivemind referral-invite shape).
        RULE_RELATIONSHIP_CADENCE => 7 * 24,
        // Everything else: daily.
        _ => 24,
    }
}

/// The cadence window bucket for `rule` at `now`.
pub fn cadence_bucket(rule: &str, now: DateTime<Utc>) -> i64 {
    let secs = min_interval_hours(rule) * 3600;
    now.timestamp().div_euclid(secs.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_visible_only_is_true_by_default() {
        let item = NotifyItem::new(RULE_RESUME_BRIEF, "project", "t", "b", "k");
        assert!(
            item.user_visible_only,
            "unflagged item must default to user-visible-only"
        );
        assert!(!item.allow_agent_channel().user_visible_only);
    }

    #[test]
    fn cadence_bucket_stable_within_window() {
        let t1: DateTime<Utc> = "2026-06-09T08:00:00Z".parse().unwrap();
        let t2: DateTime<Utc> = "2026-06-09T20:00:00Z".parse().unwrap();
        let t3: DateTime<Utc> = "2026-06-10T09:00:00Z".parse().unwrap();
        assert_eq!(
            cadence_bucket(RULE_DECISION_STALENESS, t1),
            cadence_bucket(RULE_DECISION_STALENESS, t2),
            "same 24h bucket"
        );
        assert_ne!(
            cadence_bucket(RULE_DECISION_STALENESS, t1),
            cadence_bucket(RULE_DECISION_STALENESS, t3),
            "next day → next bucket"
        );
        // Weekly rule: both days share a bucket.
        assert_eq!(
            cadence_bucket(RULE_RELATIONSHIP_CADENCE, t1),
            cadence_bucket(RULE_RELATIONSHIP_CADENCE, t3)
        );
    }
}

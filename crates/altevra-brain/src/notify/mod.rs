//! P4 — Proactive notification framework (PLAN-ALIVE §P4).
//!
//! Hivemind's Source → Rule → Delivery contract ported to Altevra:
//!
//!   * [`sources`] — candidate producers (decision-staleness,
//!     relationship-cadence, resume-brief, open-proposals). All
//!     high-precision-or-silent.
//!   * [`types`] — the [`types::NotifyItem`] contract with
//!     `user_visible_only = true` BY DEFAULT (explicit opt-out only) and
//!     per-rule cadence buckets.
//!   * [`delivery`] — atomic O_EXCL claim-file dedup + per-item
//!     `domain_policies.obsidian_mirror` consult, FAIL-CLOSED (lookup error
//!     or missing policy ⇒ drop from the Obsidian path + `audit_log` row).
//!   * [`brief`] — `daily_briefing_v1` rendering: gated (vault-safe) vs
//!     private (terminal-only, `altevra brief --private`).

pub mod brief;
pub mod delivery;
pub mod sources;
pub mod types;

pub use brief::{build_brief_data, render_brief, write_vault_brief, BriefData};
pub use delivery::{deliver, Delivery, DeliveryConfig};
pub use types::{
    cadence_bucket, min_interval_hours, NotifyItem, RULE_DECISION_STALENESS, RULE_OPEN_PROPOSALS,
    RULE_RELATIONSHIP_CADENCE, RULE_RESUME_BRIEF,
};

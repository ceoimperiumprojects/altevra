//! Contract-validation harness (BUILD_TASKS T0.8).
//!
//! Asserts the live enum value sets match the locked contract in
//! `docs/architecture/contracts/P0_CONTRACTS.md` (RECONCILIATION R1/R2/R3).
//! Any drift between code and contract is a visible test failure.

use altevra_core::domain::Domain;
use altevra_core::security::Sensitivity;
use altevra_core::status::{
    CapabilityState, LifecycleState, ObjectStatus, ProposalStatus, RedactionStatus, ReviewStatus,
};
use altevra_core::template::TemplateRegistry;

fn strs<T: ToString>(v: &[T]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

#[test]
fn sensitivity_is_the_locked_six_level_ladder() {
    // R1: public < shareable < internal < confidential < secret < restricted
    let ladder = [
        Sensitivity::Public,
        Sensitivity::Shareable,
        Sensitivity::Internal,
        Sensitivity::Confidential,
        Sensitivity::Secret,
        Sensitivity::Restricted,
    ];
    assert_eq!(
        strs(&ladder),
        [
            "public",
            "shareable",
            "internal",
            "confidential",
            "secret",
            "restricted"
        ]
    );
    // total order holds in declared order
    for w in ladder.windows(2) {
        assert!(w[0] < w[1], "{} should rank below {}", w[0], w[1]);
    }
}

#[test]
fn domain_governed_set_is_the_nine_builtins() {
    // R3
    assert_eq!(
        strs(&Domain::builtins()),
        [
            "business",
            "personal",
            "project",
            "client",
            "relationship",
            "health",
            "legal",
            "financial",
            "public"
        ]
    );
}

#[test]
fn object_status_matches_contract_and_excludes_quarantined() {
    // R2: quarantined is NOT an ObjectStatus
    assert_eq!(
        strs(&ObjectStatus::known()),
        [
            "draft",
            "active",
            "superseded",
            "archived",
            "forgotten",
            "deleted_tombstone"
        ]
    );
    assert!(!strs(&ObjectStatus::known()).contains(&"quarantined".to_string()));
}

#[test]
fn redaction_status_owns_quarantined() {
    // R2: quarantined lives here
    assert_eq!(
        strs(&RedactionStatus::known()),
        ["unscanned", "clean", "redacted", "quarantined", "rejected"]
    );
}

#[test]
fn remaining_status_families_match_contract() {
    assert_eq!(
        strs(&ReviewStatus::known()),
        [
            "not_required",
            "pending_review",
            "approved",
            "rejected",
            "needs_changes",
            "expired"
        ]
    );
    assert_eq!(
        strs(&LifecycleState::known()),
        [
            "fresh",
            "due_for_review",
            "expired",
            "archived",
            "retention_due",
            "delete_due",
            "legal_hold"
        ]
    );
    assert_eq!(
        strs(&CapabilityState::known()),
        [
            "discovered",
            "installed",
            "current",
            "outdated",
            "drifted",
            "broken",
            "disabled",
            "needs_review",
            "missing",
            "conflicted",
            "unsupported"
        ]
    );
    assert_eq!(
        strs(&ProposalStatus::known()),
        [
            "proposed",
            "triaged",
            "approved",
            "applied",
            "rejected",
            "superseded",
            "withdrawn",
            "deprecated"
        ]
    );
}

#[test]
fn template_registry_has_the_nine_p0_faced_types() {
    let reg = TemplateRegistry::with_builtins();
    assert_eq!(
        reg.object_types(),
        [
            "daily_brief",
            "decision",
            "hook",
            "insight_card",
            "learning",
            "person",
            "preference",
            "skill",
            "wiki_page"
        ]
    );
}

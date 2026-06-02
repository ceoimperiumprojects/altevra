//! Lifecycle / retention deriver (P0.8 T8.4, working draft §6.5/§6.6, R-EPH/D7).
//!
//! A PURE function (no DB, no clock, no deletion) that computes an object's
//! [`LifecycleState`] from its envelope + the resolved domain-policy retention
//! horizons + legal-hold. The brain's lifecycle job calls this to decide what to
//! soft-archive / purge / review — but the DECISION is derived here and the
//! destructive action is always review-gated (R4) downstream. Legal hold has
//! precedence over everything (D7): a held object is never delete-eligible.

use crate::envelope::Envelope;
use crate::status::{LifecycleState, ObjectStatus};
use chrono::{DateTime, Duration, Utc};

/// Derive the lifecycle state. Precedence (most actionable first): legal-hold →
/// already-archived → hard-expiry delete-due → content-expired → retention-due →
/// due-for-review → fresh.
pub fn derive_lifecycle_state(
    env: &Envelope,
    soft_ttl_days: Option<i64>,
    hard_expiry_days: Option<i64>,
    legal_hold: bool,
    now: DateTime<Utc>,
) -> LifecycleState {
    // D7: legal hold overrides retention/deletion entirely.
    if legal_hold {
        return LifecycleState::LegalHold;
    }
    if matches!(env.status, ObjectStatus::Archived) {
        return LifecycleState::Archived;
    }
    // Hard expiry (retention horizon passed) → destructive, review-gated.
    if let Some(d) = hard_expiry_days {
        if d >= 0 && now > env.created_at + Duration::days(d) {
            return LifecycleState::DeleteDue;
        }
    }
    // Content validity window passed.
    if let Some(vu) = env.valid_until {
        if now > vu {
            return LifecycleState::Expired;
        }
    }
    // Soft TTL (stale) → soft-archive candidate.
    if let Some(d) = soft_ttl_days {
        if d >= 0 && now > env.updated_at + Duration::days(d) {
            return LifecycleState::RetentionDue;
        }
    }
    // Scheduled review reached.
    if let Some(ra) = env.review_after {
        if now > ra {
            return LifecycleState::DueForReview;
        }
    }
    LifecycleState::Fresh
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::{Provenance, ProvenanceOrigin};

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn env_at(created_days_ago: i64, updated_days_ago: i64) -> Envelope {
        let mut e = Envelope::new(
            "x",
            "learning",
            now() - Duration::days(created_days_ago),
            Provenance::new(ProvenanceOrigin::PavleDirect),
        );
        e.updated_at = now() - Duration::days(updated_days_ago);
        e
    }

    #[test]
    fn legal_hold_overrides_everything() {
        // Even a long-expired object under legal hold is held, never delete-due.
        let e = env_at(9000, 9000);
        assert_eq!(
            derive_lifecycle_state(&e, Some(30), Some(365), true, now()),
            LifecycleState::LegalHold
        );
    }

    #[test]
    fn hard_expiry_is_delete_due() {
        let e = env_at(400, 400);
        assert_eq!(
            derive_lifecycle_state(&e, Some(30), Some(365), false, now()),
            LifecycleState::DeleteDue
        );
    }

    #[test]
    fn soft_ttl_is_retention_due() {
        // created recently, but not updated for longer than the soft TTL.
        let e = env_at(40, 40);
        assert_eq!(
            derive_lifecycle_state(&e, Some(30), None, false, now()),
            LifecycleState::RetentionDue
        );
    }

    #[test]
    fn valid_until_past_is_expired() {
        let mut e = env_at(1, 1);
        e.valid_until = Some(now() - Duration::days(1));
        assert_eq!(
            derive_lifecycle_state(&e, Some(30), None, false, now()),
            LifecycleState::Expired
        );
    }

    #[test]
    fn review_after_past_is_due_for_review() {
        let mut e = env_at(1, 1);
        e.review_after = Some(now() - Duration::days(1));
        assert_eq!(
            derive_lifecycle_state(&e, Some(365), None, false, now()),
            LifecycleState::DueForReview
        );
    }

    #[test]
    fn fresh_when_nothing_triggers() {
        let e = env_at(1, 1);
        assert_eq!(
            derive_lifecycle_state(&e, Some(365), Some(3650), false, now()),
            LifecycleState::Fresh
        );
    }

    #[test]
    fn permanent_domain_never_delete_due() {
        // permanent retention = no hard_expiry → never DeleteDue however old.
        let e = env_at(9000, 9000);
        let state = derive_lifecycle_state(&e, None, None, false, now());
        assert_ne!(state, LifecycleState::DeleteDue);
    }
}

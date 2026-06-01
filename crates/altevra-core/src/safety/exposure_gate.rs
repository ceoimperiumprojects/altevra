//! ExposureGate — the single read/exposure path (§2.3, R1).
//!
//! An item is exposed iff ALL hold (monotone intersection):
//!  - `sensitivity_level <= ceiling`  (R1: ONLY the level is compared)
//!  - `item.domains ⊆ request.domain_scope`
//!  - `redaction_status >= min_redaction`
//!  - status is default-readable (unless `include_history`)
//!  - object is packet-eligible
//!
//! Deny reasons are COARSE and sensitivity-aware: they never reveal the
//! existence, count, or type of a higher-classified item (§2.13 side-channel).

use crate::domain::Domain;
use crate::envelope::Envelope;
use crate::security::Sensitivity;
use crate::status::{ObjectStatus, RedactionStatus};

/// What the caller is allowed to see.
#[derive(Debug, Clone)]
pub struct ExposureRequest {
    /// The maximum sensitivity LEVEL the caller may receive.
    pub sensitivity_ceiling: Sensitivity,
    /// Domains the caller is scoped to. An item must be a subset of these.
    pub domain_scope: Vec<Domain>,
    /// Minimum acceptable redaction status (default: `Clean`/`Redacted` ok).
    pub min_redaction: RedactionStatus,
    /// Allow superseded/archived/forgotten objects (history intents only).
    pub include_history: bool,
}

impl ExposureRequest {
    /// A conservative default: internal ceiling, business+project scope, must be
    /// scanned, current objects only.
    pub fn default_work() -> Self {
        Self {
            sensitivity_ceiling: Sensitivity::Internal,
            domain_scope: vec![Domain::Business, Domain::Project, Domain::Public],
            min_redaction: RedactionStatus::Clean,
            include_history: false,
        }
    }
}

/// The gate's verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExposureDecision {
    Allow,
    Deny(DenyReason),
}

impl ExposureDecision {
    pub fn is_allowed(&self) -> bool {
        matches!(self, ExposureDecision::Allow)
    }
}

/// Coarse, non-leaking deny reasons (§2.13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DenyReason {
    /// Above the caller's ceiling — never says how far above or what it is.
    OverSensitivityCeiling,
    /// Outside the caller's domain scope.
    OutOfScope,
    /// Superseded/archived/forgotten/deleted and history not requested.
    NotCurrent,
    /// Text not sufficiently redacted for exposure (fail-closed).
    RedactionInsufficient,
}

impl DenyReason {
    /// A coarse string safe to surface in an ExclusionRecord — never leaks the
    /// existence/count/type of protected items.
    pub fn code(&self) -> &'static str {
        match self {
            DenyReason::OverSensitivityCeiling => "items_above_ceiling_omitted",
            DenyReason::OutOfScope => "out_of_scope",
            DenyReason::NotCurrent => "not_current",
            DenyReason::RedactionInsufficient => "redaction_insufficient",
        }
    }
}

/// `min_redaction` ordering: Unscanned < Clean ≈ Redacted < (Quarantined/Rejected
/// are never exposable). We treat Clean and Redacted as both exposable; anything
/// quarantined/rejected/unscanned is not.
fn redaction_exposable(status: &RedactionStatus) -> bool {
    matches!(status, RedactionStatus::Clean | RedactionStatus::Redacted)
}

pub struct ExposureGate;

impl ExposureGate {
    /// Decide whether an item may be exposed to a request. `redaction` is the
    /// item's text redaction status and is MANDATORY — there is no fail-open
    /// "not applicable" path (R11 #8: passing `None` let unscanned items leak).
    /// A genuinely text-free structural object must pass `RedactionStatus::Clean`
    /// explicitly; anything not `clean`/`redacted` is denied (fail-closed).
    pub fn decide(
        envelope: &Envelope,
        redaction: &RedactionStatus,
        request: &ExposureRequest,
    ) -> ExposureDecision {
        // Existence-protective gates run FIRST. An item above the caller's ceiling
        // or out of scope must deny with a reason that reveals nothing — even if it
        // ALSO failed lifecycle. Checking lifecycle first leaked the id/type of an
        // over-ceiling-AND-superseded item through the (benign) NotCurrent branch
        // of the packet compiler (R11 re-verify). So ceiling + scope precede
        // lifecycle + redaction.

        // 1. sensitivity ceiling — ONLY the level is compared (R1)
        if !envelope
            .sensitivity
            .within_ceiling(&request.sensitivity_ceiling)
        {
            return ExposureDecision::Deny(DenyReason::OverSensitivityCeiling);
        }

        // 2. domain scope — every domain of the item must be allowed
        for d in envelope.all_domains() {
            if !request.domain_scope.contains(&d) {
                return ExposureDecision::Deny(DenyReason::OutOfScope);
            }
        }

        // 3. lifecycle (unless history requested)
        if !request.include_history && !envelope.status.is_default_readable() {
            // exception: Draft is readable; everything else non-default is hidden
            if !matches!(envelope.status, ObjectStatus::Draft | ObjectStatus::Active) {
                return ExposureDecision::Deny(DenyReason::NotCurrent);
            }
        }

        // 4. redaction — mandatory, fail-closed: only clean/redacted is exposable.
        if !redaction_exposable(redaction) {
            return ExposureDecision::Deny(DenyReason::RedactionInsufficient);
        }

        ExposureDecision::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::{Provenance, ProvenanceOrigin};
    use chrono::{DateTime, Utc};

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn item(domain: Domain, sensitivity: Sensitivity) -> Envelope {
        let mut e = Envelope::new(
            "i",
            "decision",
            now(),
            Provenance::new(ProvenanceOrigin::PavleDirect),
        );
        e.domain = domain;
        e.sensitivity = sensitivity;
        e
    }

    #[test]
    fn business_internal_passes_work_request() {
        let e = item(Domain::Business, Sensitivity::Internal);
        let d = ExposureGate::decide(
            &e,
            &RedactionStatus::Clean,
            &ExposureRequest::default_work(),
        );
        assert!(d.is_allowed());
    }

    #[test]
    fn personal_health_excluded_from_work_packet() {
        // THE primary leak test: a restricted health object in a work request.
        let e = item(Domain::Health, Sensitivity::Restricted);
        let d = ExposureGate::decide(
            &e,
            &RedactionStatus::Clean,
            &ExposureRequest::default_work(),
        );
        // denied for BOTH ceiling and scope; ceiling is checked first.
        assert_eq!(
            d,
            ExposureDecision::Deny(DenyReason::OverSensitivityCeiling)
        );
        // the reason code does not reveal it's a health object
        if let ExposureDecision::Deny(r) = d {
            assert_eq!(r.code(), "items_above_ceiling_omitted");
        }
    }

    #[test]
    fn out_of_scope_domain_denied() {
        // internal sensitivity (within ceiling) but a domain not in scope
        let e = item(Domain::Client, Sensitivity::Internal);
        let d = ExposureGate::decide(
            &e,
            &RedactionStatus::Clean,
            &ExposureRequest::default_work(),
        );
        assert_eq!(d, ExposureDecision::Deny(DenyReason::OutOfScope));
    }

    #[test]
    fn superseded_excluded_by_default_but_shown_in_history() {
        let mut e = item(Domain::Business, Sensitivity::Internal);
        e.status = ObjectStatus::Superseded;
        let mut req = ExposureRequest::default_work();
        assert_eq!(
            ExposureGate::decide(&e, &RedactionStatus::Clean, &req),
            ExposureDecision::Deny(DenyReason::NotCurrent)
        );
        req.include_history = true;
        assert!(ExposureGate::decide(&e, &RedactionStatus::Clean, &req).is_allowed());
    }

    #[test]
    fn unscanned_text_is_fail_closed() {
        let e = item(Domain::Business, Sensitivity::Internal);
        let d = ExposureGate::decide(
            &e,
            &RedactionStatus::Unscanned,
            &ExposureRequest::default_work(),
        );
        assert_eq!(d, ExposureDecision::Deny(DenyReason::RedactionInsufficient));
    }

    #[test]
    fn higher_ceiling_admits_confidential() {
        let e = item(Domain::Business, Sensitivity::Confidential);
        let mut req = ExposureRequest::default_work();
        req.sensitivity_ceiling = Sensitivity::Confidential;
        assert!(ExposureGate::decide(&e, &RedactionStatus::Clean, &req).is_allowed());
    }
}

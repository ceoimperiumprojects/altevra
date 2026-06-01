//! PreWriteSafetyGate — the single pre-write choke point (BUILD_TASKS T1.8,
//! working draft §2.5, RECONCILIATION R1/R13).
//!
//! Pipeline: detect → redact → classify → template-gate → decide. Returns a
//! [`Guarded`] result the caller persists. Fail-closed:
//!  - raw secrets NEVER survive (redacted; only a fingerprint sighting persists);
//!  - untagged / non-conforming writes are QUARANTINED (TAG-1/TEMPLATE-1);
//!  - on any uncertainty, sensitivity defaults UP.
//!
//! Lives in `altevra-secrets` (not `altevra-core`) because it composes the local
//! secret detectors here with the template/classification types from core —
//! `secrets` already depends on `core`, so this avoids a dependency cycle.

use crate::detector::{detect_secrets, SecretKind};
use crate::redactor::redact_with;
use altevra_core::domain::RiskTag;
use altevra_core::envelope::Envelope;
use altevra_core::security::Sensitivity;
use altevra_core::status::RedactionStatus;
use altevra_core::template::gate::{GateOutcome, TemplateGate};
use altevra_core::template::TemplateRegistry;
use sha2::{Digest, Sha256};

/// A secret detection record — fingerprint + metadata ONLY, never the value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretSighting {
    pub secret_kind: String,
    /// sha256[:12] of the value — dedup/audit, non-reversing.
    pub fingerprint: String,
    /// `redacted` | `rejected`.
    pub action: String,
}

/// The result of guarding a write. The caller persists `value` (the redacted
/// text), records `sightings`, and respects `redaction_status` / `quarantined`.
#[derive(Debug, Clone)]
pub struct Guarded {
    /// Text safe to persist (secrets/PII replaced with placeholders).
    pub value: String,
    pub redaction_status: RedactionStatus,
    pub sensitivity: Sensitivity,
    pub risk_tags: Vec<RiskTag>,
    pub sightings: Vec<SecretSighting>,
    /// Template/tag conformance outcome (R13).
    pub template: GateOutcome,
    /// True if this write must go to quarantine (review), not the live store.
    pub quarantined: bool,
    /// Human-readable reasons when quarantined.
    pub reasons: Vec<String>,
}

impl Guarded {
    pub fn is_safe_to_persist(&self) -> bool {
        !self.quarantined
            && !matches!(
                self.redaction_status,
                RedactionStatus::Unscanned | RedactionStatus::Rejected
            )
    }
}

fn fingerprint(value: &str) -> String {
    let mut h = Sha256::new();
    h.update(value.as_bytes());
    let digest = h.finalize();
    hex::encode(&digest[..6]) // 12 hex chars
}

fn secret_kind_label(kind: SecretKind) -> &'static str {
    match kind {
        SecretKind::OpenAIKey => "openai",
        SecretKind::AnthropicKey => "anthropic",
        SecretKind::AwsAccessKey => "aws",
        SecretKind::GitHubToken => "github",
        SecretKind::SlackToken => "slack",
        SecretKind::GenericApiKey => "generic",
        SecretKind::JwtToken => "jwt",
        SecretKind::PrivateKey => "pem_private_key",
        SecretKind::DatabaseUrl => "db_url",
    }
}

/// A `hard_secret` is credential-class material that must never be stored raw
/// even redacted-in-place is mandatory (private keys / cloud roots / db creds).
fn is_hard_secret(kind: SecretKind) -> bool {
    matches!(
        kind,
        SecretKind::PrivateKey
            | SecretKind::AwsAccessKey
            | SecretKind::DatabaseUrl
            | SecretKind::AnthropicKey
            | SecretKind::OpenAIKey
            | SecretKind::GitHubToken
    )
}

/// Minimal P0 PII detection (T1.7): emails. Extended (phone/IBAN/card) later.
fn detect_emails(text: &str) -> Vec<(usize, usize)> {
    // simple, allocation-light scan; good enough for the P0 PII flag.
    let mut spans = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'@' {
            // walk back over local part
            let mut start = i;
            while start > 0 && is_email_char(bytes[start - 1]) {
                start -= 1;
            }
            // walk forward over domain
            let mut end = i + 1;
            let mut saw_dot = false;
            while end < bytes.len() && (is_email_char(bytes[end]) || bytes[end] == b'.') {
                if bytes[end] == b'.' {
                    saw_dot = true;
                }
                end += 1;
            }
            if start < i && end > i + 1 && saw_dot {
                spans.push((start, end));
            }
            i = end;
        } else {
            i += 1;
        }
    }
    spans
}

fn is_email_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'%' | b'+' | b'-')
}

/// The single pre-write guard. `body` is the markdown/text being written;
/// `envelope` carries domain/categories/object_type; `present_frontmatter_keys`
/// are the keys actually present (for TemplateGate).
pub fn ingest_guard(
    body: &str,
    envelope: &Envelope,
    present_frontmatter_keys: &[String],
    registry: &TemplateRegistry,
) -> Guarded {
    // ---- 1. detect + redact secrets ----
    let matches = detect_secrets(body);
    let mut sightings = Vec::new();
    let mut risk_tags = Vec::new();
    let mut had_hard = false;
    let mut value = body.to_string();

    if !matches.is_empty() {
        // redact ALL detected secrets with a typed placeholder.
        value = redact_with(body, "[REDACTED]");
        for m in &matches {
            let hard = is_hard_secret(m.kind);
            had_hard |= hard;
            // PEM private keys / credentialed DB URLs are the strongest class:
            // record them as `rejected` (the value must never be stored anywhere,
            // §2.5); other detected secrets are `redacted` in place.
            let action = match m.kind {
                SecretKind::PrivateKey | SecretKind::DatabaseUrl => "rejected",
                _ => "redacted",
            };
            sightings.push(SecretSighting {
                secret_kind: secret_kind_label(m.kind).to_string(),
                fingerprint: fingerprint(&m.matched),
                action: action.to_string(),
            });
        }
        if !risk_tags.contains(&RiskTag::Credential) {
            risk_tags.push(RiskTag::Credential);
        }
    }

    // ---- 2. PII (emails) ----
    let emails = detect_emails(&value);
    if !emails.is_empty() {
        // redact back-to-front to preserve offsets
        for (s, e) in emails.iter().rev() {
            value.replace_range(*s..*e, "[REDACTED:email]");
        }
        if !risk_tags.contains(&RiskTag::ThirdPartyPii) {
            risk_tags.push(RiskTag::ThirdPartyPii);
        }
    }

    // ---- 3. classify sensitivity (rule-based; default-up) ----
    // Start from the envelope's declared sensitivity, raise for detected risk.
    let mut sensitivity = envelope.sensitivity.clone();
    if had_hard {
        // credential-class content raises to at least Confidential.
        sensitivity = sensitivity.combine(&Sensitivity::Confidential);
    }
    if !emails.is_empty() {
        sensitivity = sensitivity.combine(&Sensitivity::Confidential);
    }

    // ---- 4. redaction status ----
    let redaction_status = if matches.is_empty() && emails.is_empty() {
        RedactionStatus::Clean
    } else {
        RedactionStatus::Redacted
    };

    // ---- 5. template + mandatory-tag gate (R13) ----
    // Build a temp envelope reflecting the (possibly raised) sensitivity.
    let mut env = envelope.clone();
    env.sensitivity = sensitivity.clone();
    for rt in &risk_tags {
        if !env.risk_tags.contains(rt) {
            env.risk_tags.push(rt.clone());
        }
    }
    let gate = TemplateGate::new(registry);
    let template = gate.check(&env, &value, present_frontmatter_keys);

    let (quarantined, mut reasons) = match &template {
        GateOutcome::Pass => (false, Vec::new()),
        GateOutcome::Quarantine(rs) => (true, rs.clone()),
    };

    Guarded {
        value,
        redaction_status,
        sensitivity,
        risk_tags,
        sightings,
        template,
        quarantined,
        reasons: {
            if had_hard {
                reasons.push("credential-class secret detected and redacted".into());
            }
            reasons
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use altevra_core::envelope::{Envelope, Provenance, ProvenanceOrigin};
    use chrono::{DateTime, Utc};

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn decision_env() -> Envelope {
        let mut e = Envelope::new(
            "d1",
            "decision",
            now(),
            Provenance::new(ProvenanceOrigin::PavleDirect),
        );
        e.categories = vec!["architecture".into()];
        e
    }

    #[test]
    fn clean_conforming_write_passes() {
        let reg = TemplateRegistry::with_builtins();
        let g = ingest_guard(
            "## Decision\nShip SQLite\n## Rationale\nLocal-first",
            &decision_env(),
            &["title".into()],
            &reg,
        );
        assert_eq!(g.redaction_status, RedactionStatus::Clean);
        assert!(g.is_safe_to_persist());
        assert!(g.sightings.is_empty());
    }

    #[test]
    fn secret_is_redacted_and_never_in_value() {
        let reg = TemplateRegistry::with_builtins();
        let body = "## Decision\nkey sk-FIXTUREfixtureFIXTUREfixture0000 here\n## Rationale\nx";
        let g = ingest_guard(body, &decision_env(), &["title".into()], &reg);
        assert!(!g.value.contains("sk-FIXTUREfixtureFIXTUREfixture0000"));
        assert!(g.value.contains("[REDACTED]"));
        assert_eq!(g.redaction_status, RedactionStatus::Redacted);
        assert!(!g.sightings.is_empty());
        // fingerprint is not the value
        assert!(!g.sightings[0].fingerprint.contains("sk-"));
        // credential risk raises sensitivity
        assert!(g.sensitivity >= Sensitivity::Confidential);
    }

    #[test]
    fn untagged_write_is_quarantined() {
        let reg = TemplateRegistry::with_builtins();
        let mut e = decision_env();
        e.categories.clear(); // TAG-1 violation
        let g = ingest_guard(
            "## Decision\nx\n## Rationale\ny",
            &e,
            &["title".into()],
            &reg,
        );
        assert!(g.quarantined);
        assert!(!g.is_safe_to_persist());
    }

    #[test]
    fn email_pii_is_redacted_and_flagged() {
        let reg = TemplateRegistry::with_builtins();
        let g = ingest_guard(
            "## Decision\ncontact john.doe@example.com\n## Rationale\nx",
            &decision_env(),
            &["title".into()],
            &reg,
        );
        assert!(!g.value.contains("john.doe@example.com"));
        assert!(g.risk_tags.contains(&RiskTag::ThirdPartyPii));
    }

    #[test]
    fn private_key_redacted() {
        let reg = TemplateRegistry::with_builtins();
        let body = "## Decision\n-----BEGIN PRIVATE KEY-----\n## Rationale\nx";
        let g = ingest_guard(body, &decision_env(), &["title".into()], &reg);
        assert!(!g.value.contains("BEGIN PRIVATE KEY"));
        assert!(g.sightings.iter().any(|s| s.secret_kind == "pem_private_key"));
    }
}

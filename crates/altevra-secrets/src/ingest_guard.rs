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
use crate::pii::{detect_pii, PiiKind};
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
        SecretKind::StripeKey => "stripe",
        SecretKind::AwsAccessKey => "aws",
        SecretKind::GoogleApiKey => "google",
        SecretKind::NpmToken => "npm",
        SecretKind::GitHubToken => "github",
        SecretKind::SlackToken => "slack",
        SecretKind::SlackWebhook => "slack_webhook",
        SecretKind::GenericApiKey => "generic",
        SecretKind::BearerToken => "bearer",
        SecretKind::JwtToken => "jwt",
        SecretKind::PrivateKey => "pem_private_key",
        SecretKind::DatabaseUrl => "db_url",
    }
}

/// A `hard_secret` is credential-class material that must never be stored raw —
/// redacted-in-place is mandatory (private keys / cloud roots / db creds / live
/// API keys). Covers every kind that is a usable live credential.
fn is_hard_secret(kind: SecretKind) -> bool {
    matches!(
        kind,
        SecretKind::PrivateKey
            | SecretKind::AwsAccessKey
            | SecretKind::DatabaseUrl
            | SecretKind::AnthropicKey
            | SecretKind::OpenAIKey
            | SecretKind::StripeKey
            | SecretKind::GoogleApiKey
            | SecretKind::NpmToken
            | SecretKind::GitHubToken
            | SecretKind::SlackToken
            | SecretKind::SlackWebhook
            | SecretKind::BearerToken
    )
}

/// Email PII detection (T1.7). Phone/IBAN/card detection lives in [`crate::pii`]
/// and is applied alongside this in [`guard_text`].
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

/// Text-only guard: secret + PII redaction + sensitivity classification, with
/// NO template/tag enforcement. Used for high-volume activity text (hook-captured
/// turns, session content) that isn't a templated faced object but MUST still be
/// scrubbed before persistence (BUILD_TASKS T1.13). `declared_sensitivity` is the
/// caller's starting point; the guard only ever RAISES it (default-up).
#[derive(Debug, Clone)]
pub struct GuardedText {
    pub value: String,
    pub redaction_status: RedactionStatus,
    pub sensitivity: Sensitivity,
    pub risk_tags: Vec<RiskTag>,
    pub sightings: Vec<SecretSighting>,
}

pub fn guard_text(body: &str, declared_sensitivity: Sensitivity) -> GuardedText {
    // ---- 1. detect + redact secrets ----
    let matches = detect_secrets(body);
    let mut sightings = Vec::new();
    let mut risk_tags = Vec::new();
    let mut had_hard = false;
    let mut value = body.to_string();

    if !matches.is_empty() {
        value = redact_with(body, "[REDACTED]");
        for m in &matches {
            had_hard |= is_hard_secret(m.kind);
            // PEM private keys / credentialed DB URLs are the strongest class:
            // recorded as `rejected` (value must never be stored, §2.5); others
            // are `redacted` in place.
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
        risk_tags.push(RiskTag::Credential);
    }

    // ---- 2. PII: emails ----
    let emails = detect_emails(&value);
    if !emails.is_empty() {
        for (s, e) in emails.iter().rev() {
            value.replace_range(*s..*e, "[REDACTED:email]");
        }
        risk_tags.push(RiskTag::ThirdPartyPii);
    }

    // ---- 2b. PII: phone / IBAN / payment-card (R11 — was email-only) ----
    let pii = detect_pii(&value);
    if !pii.is_empty() {
        let mut had_phone = false;
        let mut had_financial = false;
        for m in pii.iter().rev() {
            let placeholder = match m.kind {
                PiiKind::Phone => {
                    had_phone = true;
                    "[REDACTED:phone]"
                }
                PiiKind::Iban => {
                    had_financial = true;
                    "[REDACTED:iban]"
                }
                PiiKind::CreditCard => {
                    had_financial = true;
                    "[REDACTED:card]"
                }
            };
            value.replace_range(m.start..m.end, placeholder);
        }
        if had_phone && !risk_tags.contains(&RiskTag::ThirdPartyPii) {
            risk_tags.push(RiskTag::ThirdPartyPii);
        }
        if had_financial && !risk_tags.contains(&RiskTag::Financial) {
            risk_tags.push(RiskTag::Financial);
        }
    }

    // ---- 2c. content high-water markers (coarse P0 net; LLM refines in P0.5) ----
    for rt in high_water_keywords(&value) {
        if !risk_tags.contains(&rt) {
            risk_tags.push(rt);
        }
    }

    let redacted_any = !matches.is_empty() || !emails.is_empty() || !pii.is_empty();

    // ---- 3. classify sensitivity (default-UP, fail-closed) ----
    // A high-water risk tag (health/relationship/legal/financial) forces the
    // top of the ladder — this is the personal-first parity rule (R11 #4): such
    // content must never default-down to Internal and leak into work packets.
    let mut sensitivity = declared_sensitivity;
    let has_high_water = risk_tags.iter().any(|t| {
        matches!(
            t,
            RiskTag::Health | RiskTag::Relationship | RiskTag::Legal | RiskTag::Financial
        )
    });
    if has_high_water {
        sensitivity = sensitivity.combine(&Sensitivity::Restricted);
    } else if had_hard || !risk_tags.is_empty() {
        // credential / third-party-PII present but no high-water domain marker.
        sensitivity = sensitivity.combine(&Sensitivity::Confidential);
    }

    // ---- 4. redaction status ----
    let redaction_status = if redacted_any {
        RedactionStatus::Redacted
    } else {
        RedactionStatus::Clean
    };

    GuardedText {
        value,
        redaction_status,
        sensitivity,
        risk_tags,
        sightings,
    }
}

/// Coarse keyword net for personal high-water content that carries NO secret and
/// NO structured PII (e.g. "my HIV diagnosis", "raskid sa devojkom"). Returns the
/// risk tags implied so the classifier can raise sensitivity to `Restricted`.
/// Deliberately small + high-precision; the P0.5 LLM classifier supersedes it.
/// Fail-closed bias: a false positive only over-protects.
fn high_water_keywords(text: &str) -> Vec<RiskTag> {
    let lc = text.to_lowercase();
    let mut tags = Vec::new();
    const HEALTH: &[&str] = &[
        "diagnosis",
        "diagnosed",
        "therapist",
        "psychiatrist",
        "antidepressant",
        "chemotherapy",
        " hiv ",
        "cancer",
        "depression",
        "anxiety disorder",
        "suicidal",
        "suicide",
        "abortion",
        "miscarriage",
        "prozac",
        "xanax",
        "zoloft",
        "dijagnoza",
        "terapeut",
        "psihijatar",
        "antidepresiv",
        "depresij",
        "anksioznost",
        "samoubist",
    ];
    const RELATIONSHIP: &[&str] = &[
        "my girlfriend",
        "my boyfriend",
        "my partner",
        "my wife",
        "my husband",
        "breakup",
        "divorce",
        "raskid",
        "devojka mi",
        "moja devojka",
        "moj dečko",
        "moja žena",
        "moj muž",
    ];
    const LEGAL: &[&str] = &["lawsuit", "attorney-client", "tužba", "advokat"];
    if HEALTH.iter().any(|k| lc.contains(k)) {
        tags.push(RiskTag::Health);
    }
    if RELATIONSHIP.iter().any(|k| lc.contains(k)) {
        tags.push(RiskTag::Relationship);
    }
    if LEGAL.iter().any(|k| lc.contains(k)) {
        tags.push(RiskTag::Legal);
    }
    tags
}

/// The single pre-write guard for templated FACED objects. `body` is the markdown
/// being written; `envelope` carries domain/categories/object_type;
/// `present_frontmatter_keys` are the keys present (for TemplateGate). Composes
/// [`guard_text`] (secret/PII/classify) with the R13 template + mandatory-tag gate.
pub fn ingest_guard(
    body: &str,
    envelope: &Envelope,
    present_frontmatter_keys: &[String],
    registry: &TemplateRegistry,
) -> Guarded {
    let gt = guard_text(body, envelope.sensitivity.clone());
    let GuardedText {
        value,
        redaction_status,
        mut sensitivity,
        risk_tags,
        sightings,
    } = gt;

    // ---- domain-driven escalation (R3 most-restrictive, fail-closed) ----
    // A high-water domain (personal/relationship/health/legal/financial/client)
    // forces the top of the ladder regardless of content — so even prose with no
    // detectable secret/PII cannot default-down and leak (R11 #4).
    let high_water_domain =
        envelope.domain.is_high_water() || envelope.domains.iter().any(|d| d.is_high_water());
    if high_water_domain {
        sensitivity = sensitivity.combine(&Sensitivity::Restricted);
    }

    // ---- template + mandatory-tag gate (R13) ----
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

    let (mut quarantined, mut reasons) = match &template {
        GateOutcome::Pass => (false, Vec::new()),
        GateOutcome::Quarantine(rs) => (true, rs.clone()),
    };
    // A `rejected`-class sighting (PEM / db-url credentials) means a credential
    // was present — force quarantine so a human reviews it, never silently store
    // (R11 Codex #7: is_safe_to_persist must not stay true on a rejected secret).
    if sightings.iter().any(|s| s.action == "rejected") {
        quarantined = true;
        reasons.push("credential-class secret detected and redacted".into());
    }

    Guarded {
        value,
        redaction_status,
        sensitivity,
        risk_tags,
        sightings,
        template,
        quarantined,
        reasons,
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
    fn phone_and_iban_redacted_and_classified_restricted() {
        // R11 #5/#4: a turn with phone + IBAN but NO email/secret used to persist
        // Clean/Internal. Now PII is redacted and Financial high-water → Restricted.
        let g = guard_text(
            "call Elena at +381 64 123 4567, IBAN GB82WEST12345698765432",
            Sensitivity::Internal,
        );
        assert!(
            !g.value.contains("+381 64 123 4567"),
            "phone leaked: {}",
            g.value
        );
        assert!(!g.value.contains("GB82WEST12345698765432"), "iban leaked");
        assert_eq!(g.redaction_status, RedactionStatus::Redacted);
        assert!(g.risk_tags.contains(&RiskTag::Financial));
        assert_eq!(g.sensitivity, Sensitivity::Restricted);
    }

    #[test]
    fn health_prose_classified_restricted() {
        // R11 #4: pure health prose with no secret/PII must not default-down.
        let g = guard_text(
            "my HIV diagnosis is under control now",
            Sensitivity::Internal,
        );
        assert!(g.risk_tags.contains(&RiskTag::Health));
        assert_eq!(g.sensitivity, Sensitivity::Restricted);
        // A plain work sentence stays at the declared level.
        let work = guard_text(
            "the staging server is healthy and deploys clean",
            Sensitivity::Internal,
        );
        assert_eq!(work.sensitivity, Sensitivity::Internal);
        assert_eq!(work.redaction_status, RedactionStatus::Clean);
    }

    #[test]
    fn high_water_domain_forces_restricted() {
        // R11 #4: a high-water DOMAIN (health) forces Restricted even for benign
        // prose with no detectable marker at all.
        let reg = TemplateRegistry::with_builtins();
        let mut e = decision_env();
        e.domain = altevra_core::domain::Domain::Health;
        let g = ingest_guard(
            "## Decision\nroutine note\n## Rationale\nnothing sensitive in the words",
            &e,
            &["title".into()],
            &reg,
        );
        assert_eq!(g.sensitivity, Sensitivity::Restricted);
    }

    #[test]
    fn rejected_secret_forces_quarantine() {
        // R11 Codex #7: a credential-class (db-url) sighting must quarantine.
        let reg = TemplateRegistry::with_builtins();
        let g = ingest_guard(
            "## Decision\ndb postgres://u:longpasswordvalue123@h/db\n## Rationale\nx",
            &decision_env(),
            &["title".into()],
            &reg,
        );
        assert!(g.sightings.iter().any(|s| s.action == "rejected"));
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
        assert!(g
            .sightings
            .iter()
            .any(|s| s.secret_kind == "pem_private_key"));
    }
}

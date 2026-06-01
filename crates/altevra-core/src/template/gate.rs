//! TemplateGate (RECONCILIATION R13, invariants TAG-1 / TEMPLATE-1).
//!
//! Runs inside (or right after) the PreWriteSafetyGate. A write must satisfy:
//!
//! - TAG-1: a resolved governed `domain` + ≥1 governed `category`.
//! - TEMPLATE-1: if the type is templated, its required frontmatter keys and
//!   body sections must be present.
//!
//! Otherwise the write is QUARANTINED (not silently stored) — this is what
//! stops malformed content and keeps everything searchable.

use super::TemplateRegistry;
use crate::domain::Domain;
use crate::envelope::Envelope;

/// Outcome of the template/tag check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateOutcome {
    /// Conforms — safe to persist.
    Pass,
    /// Does not conform — quarantine with human-readable reasons.
    Quarantine(Vec<String>),
}

impl GateOutcome {
    pub fn is_pass(&self) -> bool {
        matches!(self, GateOutcome::Pass)
    }
}

pub struct TemplateGate<'a> {
    registry: &'a TemplateRegistry,
}

impl<'a> TemplateGate<'a> {
    pub fn new(registry: &'a TemplateRegistry) -> Self {
        Self { registry }
    }

    /// Validate an object before persistence.
    ///
    /// * `envelope` — carries domain + categories (TAG-1).
    /// * `body` — the markdown body, checked for required sections.
    /// * `present_frontmatter_keys` — keys actually present on the write.
    pub fn check(
        &self,
        envelope: &Envelope,
        body: &str,
        present_frontmatter_keys: &[String],
    ) -> GateOutcome {
        let mut reasons = Vec::new();

        // TAG-1: governed domain (not Other) — an unknown domain is review-gated.
        if let Domain::Other(d) = &envelope.domain {
            reasons.push(format!(
                "domain '{d}' is not in the governed set (new domains are review-gated)"
            ));
        }

        // TAG-1: at least one governed category.
        let min = self
            .registry
            .get(&envelope.object_type)
            .map(|t| t.min_categories)
            .unwrap_or(1); // default: every durable object needs ≥1 category
        if envelope.categories.len() < min {
            reasons.push(format!(
                "untagged: {} category(ies) present, {} required (TAG-1)",
                envelope.categories.len(),
                min
            ));
        }

        // TEMPLATE-1: structural conformance for templated (faced) types.
        if let Some(t) = self.registry.get(&envelope.object_type) {
            for key in &t.required_frontmatter {
                if !present_frontmatter_keys.iter().any(|k| k == key) {
                    reasons.push(format!("missing required frontmatter key: '{key}'"));
                }
            }
            for section in &t.required_sections {
                if !body.contains(section.as_str()) {
                    reasons.push(format!("missing required body section: '{section}'"));
                }
            }
        }

        if reasons.is_empty() {
            GateOutcome::Pass
        } else {
            GateOutcome::Quarantine(reasons)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::{Envelope, Provenance, ProvenanceOrigin};
    use crate::security::Sensitivity;
    use chrono::{DateTime, Utc};

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn env(object_type: &str) -> Envelope {
        Envelope::new(
            "o1",
            object_type,
            now(),
            Provenance::new(ProvenanceOrigin::PavleDirect),
        )
    }

    #[test]
    fn untagged_object_is_quarantined() {
        let reg = TemplateRegistry::with_builtins();
        let gate = TemplateGate::new(&reg);
        let e = env("decision"); // no categories
        let out = gate.check(&e, "## Decision\nx\n## Rationale\ny", &["title".into()]);
        assert!(!out.is_pass());
    }

    #[test]
    fn conforming_decision_passes() {
        let reg = TemplateRegistry::with_builtins();
        let gate = TemplateGate::new(&reg);
        let mut e = env("decision");
        e.categories = vec!["gtm".into()];
        let out = gate.check(
            &e,
            "## Decision\nShip it\n## Rationale\nBecause",
            &["title".into()],
        );
        assert_eq!(out, GateOutcome::Pass);
    }

    #[test]
    fn missing_section_quarantines() {
        let reg = TemplateRegistry::with_builtins();
        let gate = TemplateGate::new(&reg);
        let mut e = env("decision");
        e.categories = vec!["gtm".into()];
        // missing "## Rationale"
        let out = gate.check(&e, "## Decision\nShip it", &["title".into()]);
        match out {
            GateOutcome::Quarantine(reasons) => {
                assert!(reasons.iter().any(|r| r.contains("## Rationale")));
            }
            _ => panic!("expected quarantine"),
        }
    }

    #[test]
    fn unknown_domain_is_flagged() {
        let reg = TemplateRegistry::with_builtins();
        let gate = TemplateGate::new(&reg);
        let mut e = env("learning");
        e.categories = vec!["x".into()];
        e.domain = Domain::Other("fitness".into());
        let out = gate.check(&e, "## Learning\nstuff", &["title".into()]);
        match out {
            GateOutcome::Quarantine(reasons) => {
                assert!(reasons.iter().any(|r| r.contains("governed set")));
            }
            _ => panic!("expected quarantine for ungoverned domain"),
        }
    }

    #[test]
    fn non_templated_type_still_needs_a_category() {
        let reg = TemplateRegistry::with_builtins();
        let gate = TemplateGate::new(&reg);
        let e = env("system_event"); // no template
        let out = gate.check(&e, "", &[]);
        assert!(!out.is_pass()); // TAG-1 applies to all durable objects
    }
}

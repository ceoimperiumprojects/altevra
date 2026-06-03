//! Shared "what role classifies this object?" decision (SI-7 single source of truth).
//!
//! Today two call sites duplicated the same rule: the auto-categorizer (jobs.rs)
//! had a `domain.is_high_water() || content_is_high_water(text)` ladder, and the
//! resident runtime had only the mode-declared role with no content escalation.
//! That meant a high-water OBJECT classified by a mode whose role is
//! `CheapWorker` could still be routed to the cloud — even though the auto-
//! categorizer would route the same object local.
//!
//! [`role_for_object`] is the one rule both paths now share:
//!
//! 1. If the object's `Domain` is high-water (Personal/Relationship/Health/Legal/
//!    Financial/Client), route to [`ModelRole::LocalPrivate`].
//! 2. Else if the title/body looks high-water by content
//!    ([`altevra_secrets::content_is_high_water`]) — independent of any
//!    upstream domain stamp — escalate to [`ModelRole::LocalPrivate`].
//! 3. Else fall back to the caller's `default` (the cloud cheap_worker /
//!    strong_reasoner that the mode was registered with).
//!
//! When `LocalPrivate` resolves to `noop` (no local model configured), the
//! existing behavior (skip cleanly) holds — the router enforces that.

use altevra_core::Domain;
use altevra_llm::ModelRole;

/// Pick the [`ModelRole`] for an object given its declared `domain`, the title/body
/// text we are about to classify, and the role the CALLER would otherwise use.
///
/// The helper is total + pure: a caller can compose it without an `await`, and a
/// false positive only ever keeps work local (safe-by-default per SI-7).
pub fn role_for_object(domain: &Domain, text: &str, default: ModelRole) -> ModelRole {
    if domain.is_high_water() {
        return ModelRole::LocalPrivate;
    }
    if altevra_secrets::content_is_high_water(text) {
        return ModelRole::LocalPrivate;
    }
    default
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn high_water_domain_forces_local() {
        // A relationship object always goes local, no matter the default.
        assert_eq!(
            role_for_object(&Domain::Relationship, "anything", ModelRole::CheapWorker),
            ModelRole::LocalPrivate
        );
        assert_eq!(
            role_for_object(&Domain::Health, "anything", ModelRole::StrongReasoner),
            ModelRole::LocalPrivate
        );
    }

    #[test]
    fn business_domain_with_clean_text_returns_default() {
        // The cold-call list is genuinely business — no content escalation.
        assert_eq!(
            role_for_object(
                &Domain::Business,
                "ReVesta cold call list for surplus buyers in Florida",
                ModelRole::CheapWorker,
            ),
            ModelRole::CheapWorker
        );
        assert_eq!(
            role_for_object(
                &Domain::Public,
                "blog post draft about Rust async runtimes",
                ModelRole::StrongReasoner,
            ),
            ModelRole::StrongReasoner
        );
    }

    #[test]
    fn business_domain_but_high_water_content_escalates() {
        // Mislabeled object: domain=business but the body is clearly relational.
        // The content fail-safe re-routes it to LocalPrivate even though the
        // domain stamp is non-high-water (the SI-7 content fail-safe).
        let body =
            "danas sam shvatio nesto vazno — moja devojka Elena me podrzava u svemu";
        assert_eq!(
            role_for_object(&Domain::Business, body, ModelRole::CheapWorker),
            ModelRole::LocalPrivate
        );
    }
}

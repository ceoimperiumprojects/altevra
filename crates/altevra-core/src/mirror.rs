//! Imperium generated-mirror renderer (P0.8 T8.7, R10 Q-VAULT, §2.14 D4).
//!
//! Altevra may surface an object into the HUMAN Imperium vault (`~/Obsidian/
//! Imperium/`) ONLY as a `generated_mirror` — never authoritative. This is the
//! PURE policy + renderer: it decides whether an object may be mirrored and, if
//! so, produces the managed-header markdown + its relative vault path. It does NOT
//! write to disk (a separate writer wires that, path-gated) — so it is safe and
//! keyless. D4: confidential+ and high-water-domain content is NEVER mirrored as
//! plaintext; it stays local-only.

use crate::envelope::Envelope;
use crate::security::Sensitivity;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorDoc {
    /// Path relative to the Imperium vault root (numbered zone + id).
    pub relative_path: String,
    pub content: String,
}

/// Render an object's Imperium mirror, or `None` if it must NOT be mirrored
/// (confidential+ level or any high-water domain → local-only, D4).
pub fn render_mirror(env: &Envelope, title: &str, body: &str) -> Option<MirrorDoc> {
    // D4: no plaintext mirror for confidential+ ...
    if env.sensitivity.rank() >= Sensitivity::Confidential.rank() {
        return None;
    }
    // ... nor for any high-water domain (personal/health/relationship/...).
    if env.domain.is_high_water() || env.domains.iter().any(|d| d.is_high_water()) {
        return None;
    }
    let zone = match env.object_type.as_str() {
        "decision" => "08-decisions",
        "learning" => "10-learnings",
        "wiki_page" => "20-wiki",
        "insight_card" => "10-insights",
        _ => "30-objects",
    };
    let content = format!(
        "<!-- ALTEVRA_MANAGED: true -->\n\
         <!-- generated_mirror: true (Altevra is NOT the source of truth for this file) -->\n\
         <!-- source: {ty}:{id} -->\n\
         ---\n\
         type: {ty}\n\
         id: {id}\n\
         domain: {dom}\n\
         sensitivity: {sens}\n\
         ---\n\n# {title}\n\n{body}\n",
        ty = env.object_type,
        id = env.id,
        dom = env.domain,
        sens = env.sensitivity,
        title = title,
        body = body.trim(),
    );
    Some(MirrorDoc {
        relative_path: format!("{zone}/{}.md", env.id),
        content,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Domain;
    use crate::envelope::{Provenance, ProvenanceOrigin};
    use chrono::{DateTime, Utc};

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn env(ty: &str, domain: Domain, sens: Sensitivity) -> Envelope {
        let mut e = Envelope::new(
            "o1",
            ty,
            now(),
            Provenance::new(ProvenanceOrigin::PavleDirect),
        );
        e.domain = domain;
        e.sensitivity = sens;
        e
    }

    #[test]
    fn business_internal_is_mirrored_with_managed_header() {
        let m = render_mirror(
            &env("decision", Domain::Business, Sensitivity::Internal),
            "Adopt SQLite",
            "Local-first store.",
        )
        .expect("internal business decision mirrors");
        assert_eq!(m.relative_path, "08-decisions/o1.md");
        assert!(m.content.contains("ALTEVRA_MANAGED: true"));
        assert!(m.content.contains("generated_mirror: true"));
        assert!(m.content.contains("# Adopt SQLite"));
    }

    #[test]
    fn confidential_is_never_mirrored() {
        assert!(render_mirror(
            &env("decision", Domain::Business, Sensitivity::Confidential),
            "Deal terms",
            "secret",
        )
        .is_none());
    }

    #[test]
    fn high_water_domain_is_never_mirrored() {
        // health/relationship/personal stay local-only even at internal level.
        for d in [
            Domain::Health,
            Domain::Relationship,
            Domain::Personal,
            Domain::Financial,
        ] {
            assert!(
                render_mirror(&env("learning", d.clone(), Sensitivity::Internal), "t", "b")
                    .is_none(),
                "{d:?} must not mirror"
            );
        }
    }

    #[test]
    fn public_shareable_mirrors() {
        assert!(render_mirror(
            &env("wiki_page", Domain::Public, Sensitivity::Public),
            "Wiki",
            "body",
        )
        .is_some());
    }
}

//! Template + mandatory-tag system (RECONCILIATION R13).
//!
//! Every durable type with a markdown face has a canonical [`Template`]:
//! required frontmatter keys, required body sections, and a minimum number of
//! governed categories (TAG-1). Renderers render *from* the template, so
//! generated faces are structurally identical every time; writes that don't
//! conform are quarantined by the [`gate::TemplateGate`].
//!
//! This is the search substrate now that vector/semantic search is out of the
//! core path (R12): structure + governed tags = deterministic retrieval.

pub mod gate;

use crate::domain::Domain;
use crate::security::Sensitivity;
use std::collections::HashMap;

/// The contract a templated object's face must satisfy.
#[derive(Debug, Clone)]
pub struct Template {
    pub object_type: String,
    /// Frontmatter keys that MUST be present (beyond the universal envelope).
    pub required_frontmatter: Vec<String>,
    /// Markdown body sections (e.g. "## Trigger") that MUST appear.
    pub required_sections: Vec<String>,
    /// TAG-1: minimum governed categories required before persist.
    pub min_categories: usize,
    /// Default domain seeded at create when the object doesn't override.
    pub default_domain: Domain,
    /// Default sensitivity seeded at create.
    pub default_sensitivity: Sensitivity,
}

impl Template {
    fn new(
        object_type: &str,
        required_frontmatter: &[&str],
        required_sections: &[&str],
        min_categories: usize,
        default_domain: Domain,
        default_sensitivity: Sensitivity,
    ) -> Self {
        Self {
            object_type: object_type.to_string(),
            required_frontmatter: required_frontmatter.iter().map(|s| s.to_string()).collect(),
            required_sections: required_sections.iter().map(|s| s.to_string()).collect(),
            min_categories,
            default_domain,
            default_sensitivity,
        }
    }
}

/// Registry of the builtin templates. Governed: adding/altering a template is a
/// review-gated change (like a domain_policy edit).
#[derive(Debug, Clone)]
pub struct TemplateRegistry {
    templates: HashMap<String, Template>,
}

impl TemplateRegistry {
    pub fn get(&self, object_type: &str) -> Option<&Template> {
        self.templates.get(object_type)
    }

    pub fn len(&self) -> usize {
        self.templates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }

    pub fn object_types(&self) -> Vec<String> {
        let mut v: Vec<String> = self.templates.keys().cloned().collect();
        v.sort();
        v
    }

    /// The builtin templates for the P0 faced object types.
    pub fn with_builtins() -> Self {
        use Domain::*;
        use Sensitivity::*;
        let list = vec![
            // skill: P0.7 skill factory output (§5 SkillBody fields).
            Template::new(
                "skill",
                &["slug", "version", "title"],
                &[
                    "## Trigger",
                    "## Steps",
                    "## Commands",
                    "## Pitfalls",
                    "## Verification",
                ],
                1,
                Business,
                Internal,
            ),
            Template::new(
                "hook",
                &["slug", "version", "hook_type"],
                &["## Actions"],
                1,
                Business,
                Internal,
            ),
            Template::new(
                "wiki_page",
                &["topic", "title"],
                &["## Summary"],
                1,
                Business,
                Internal,
            ),
            Template::new(
                "daily_brief",
                &["date"],
                &["## Focus Today", "## What Changed", "## Next"],
                1,
                Business,
                Internal,
            ),
            Template::new(
                "decision",
                &["title"],
                &["## Decision", "## Rationale"],
                1,
                Business,
                Internal,
            ),
            Template::new(
                "learning",
                &["title"],
                &["## Learning"],
                1,
                Business,
                Internal,
            ),
            // person/preference/relationship default to high-water personal domains.
            Template::new(
                "person",
                &["name"],
                &["## Context"],
                1,
                Personal,
                Restricted,
            ),
            Template::new(
                "preference",
                &["key"],
                &["## Preference"],
                1,
                Personal,
                Confidential,
            ),
            Template::new(
                "insight_card",
                &["title"],
                &["## Insight"],
                1,
                Business,
                Internal,
            ),
        ];
        let templates = list
            .into_iter()
            .map(|t| (t.object_type.clone(), t))
            .collect();
        Self { templates }
    }
}

impl Default for TemplateRegistry {
    fn default() -> Self {
        Self::with_builtins()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nine_builtin_templates() {
        let r = TemplateRegistry::with_builtins();
        assert_eq!(r.len(), 9);
        for t in [
            "skill",
            "hook",
            "wiki_page",
            "daily_brief",
            "decision",
            "learning",
            "person",
            "preference",
            "insight_card",
        ] {
            assert!(r.get(t).is_some(), "missing template: {t}");
        }
    }

    #[test]
    fn skill_template_requires_skillbody_sections() {
        let r = TemplateRegistry::with_builtins();
        let skill = r.get("skill").unwrap();
        assert!(skill.required_sections.iter().any(|s| s == "## Steps"));
        assert!(skill
            .required_sections
            .iter()
            .any(|s| s == "## Verification"));
    }

    #[test]
    fn personal_types_default_high_water() {
        let r = TemplateRegistry::with_builtins();
        assert_eq!(
            r.get("person").unwrap().default_sensitivity,
            Sensitivity::Restricted
        );
        assert_eq!(r.get("person").unwrap().default_domain, Domain::Personal);
    }

    #[test]
    fn every_template_requires_at_least_one_category() {
        let r = TemplateRegistry::with_builtins();
        for t in r.object_types() {
            assert!(
                r.get(&t).unwrap().min_categories >= 1,
                "{t} allows untagged"
            );
        }
    }
}

//! The mandatory durable-object envelope (working draft §1.3, Constitution Law 1).
//!
//! Every durable object — DB row and/or markdown frontmatter — carries this
//! envelope. The conformance meta-test (P0.1) asserts every durable table has
//! the Required columns; this struct is the in-memory shape they map to.

use crate::domain::{Domain, RiskTag};
use crate::security::Sensitivity;
use crate::status::ObjectStatus;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Where a fact came from and how much we trust it (§1.4.6).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provenance {
    pub origin: ProvenanceOrigin,
    /// `session:<id>` | `turn:<id>` | `file:<path>` | `url:<url>` | `import:<batch>` | `object:<type>:<id>`
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source_ref: Option<String>,
    /// e.g. `agent:claude-code`, `user:pavle`
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub captured_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub captured_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub confidence_origin: Option<ConfidenceOrigin>,
}

impl Provenance {
    /// Minimal valid provenance: origin is the only required field.
    pub fn new(origin: ProvenanceOrigin) -> Self {
        Self {
            origin,
            source_ref: None,
            captured_by: None,
            captured_at: None,
            tool: None,
            confidence_origin: None,
        }
    }

    /// `imported` provenance MUST carry a source_ref (no anonymous imports, §1 FM-9).
    pub fn is_valid(&self) -> bool {
        if matches!(self.origin, ProvenanceOrigin::Imported) {
            return self.source_ref.is_some();
        }
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceOrigin {
    PavleDirect,
    AgentInferred,
    Imported,
    SystemDerived,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfidenceOrigin {
    Stated,
    Observed,
    Derived,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    Low,
    #[default]
    Medium,
    High,
}

/// The common metadata envelope mandatory on every durable object (§1.3).
/// Only the always-required + commonly-used fields are first-class here;
/// conditional fields (supersedes/superseded_by, valid_until, review_after,
/// checksum, origin_device) live as Options.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub id: String,
    /// Object type discriminator (e.g. "decision", "wiki_page", "learning").
    #[serde(rename = "type")]
    pub object_type: String,
    pub schema_version: u32,
    pub status: ObjectStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub provenance: Provenance,
    pub sensitivity: Sensitivity,
    /// Primary domain. `domains` may hold additional ones for cross-domain objects.
    pub domain: Domain,
    #[serde(default)]
    pub domains: Vec<Domain>,
    /// `scope` = project id, or `None` = global.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub scope: Option<String>,
    /// TAG-1: at least one governed category is mandatory before persist.
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub risk_tags: Vec<RiskTag>,
    #[serde(default = "default_confidence")]
    pub confidence: Confidence,
    pub revision: u32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub supersedes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub superseded_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub valid_until: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub review_after: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub origin_device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub checksum: Option<String>,
    /// `policy_version` the object's defaults were seeded from (§6 D2).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub policy_version: Option<u32>,
}

fn default_confidence() -> Confidence {
    Confidence::Medium
}

impl Envelope {
    /// A fresh envelope with the required fields set and safe defaults.
    pub fn new(
        id: impl Into<String>,
        object_type: impl Into<String>,
        now: DateTime<Utc>,
        provenance: Provenance,
    ) -> Self {
        Self {
            id: id.into(),
            object_type: object_type.into(),
            schema_version: 1,
            status: ObjectStatus::Active,
            created_at: now,
            updated_at: now,
            provenance,
            sensitivity: Sensitivity::Internal,
            domain: Domain::Business,
            domains: Vec::new(),
            scope: None,
            categories: Vec::new(),
            tags: Vec::new(),
            risk_tags: Vec::new(),
            confidence: Confidence::Medium,
            revision: 1,
            supersedes: None,
            superseded_by: None,
            valid_until: None,
            review_after: None,
            origin_device: None,
            checksum: None,
            policy_version: None,
        }
    }

    /// The full set of domains this object spans (primary + extras).
    pub fn all_domains(&self) -> Vec<Domain> {
        let mut v = vec![self.domain.clone()];
        for d in &self.domains {
            if !v.contains(d) {
                v.push(d.clone());
            }
        }
        v
    }

    /// I1 envelope completeness: required fields are non-empty + provenance valid.
    /// (Required scalar fields are non-null by the type system; this checks the
    /// runtime constraints: id non-empty, type non-empty, provenance validity.)
    pub fn is_complete(&self) -> bool {
        !self.id.is_empty()
            && !self.object_type.is_empty()
            && self.schema_version >= 1
            && self.revision >= 1
            && self.created_at <= self.updated_at
            && self.provenance.is_valid()
    }

    /// TAG-1: a durable object must carry ≥1 governed category before persist.
    pub fn is_tagged(&self) -> bool {
        !self.categories.is_empty()
    }
}

/// Implemented by any durable object so generic machinery (gates, packets,
/// index) can read its envelope without knowing the concrete type.
pub trait HasEnvelope {
    fn envelope(&self) -> &Envelope;
    fn envelope_mut(&mut self) -> &mut Envelope;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn new_envelope_is_complete_but_untagged() {
        let e = Envelope::new(
            "obj_1",
            "decision",
            now(),
            Provenance::new(ProvenanceOrigin::PavleDirect),
        );
        assert!(e.is_complete());
        assert!(!e.is_tagged()); // TAG-1: must add a category before persist
    }

    #[test]
    fn imported_requires_source_ref() {
        let mut p = Provenance::new(ProvenanceOrigin::Imported);
        assert!(!p.is_valid());
        p.source_ref = Some("import:batch_7".into());
        assert!(p.is_valid());
    }

    #[test]
    fn all_domains_dedups() {
        let mut e = Envelope::new(
            "x",
            "learning",
            now(),
            Provenance::new(ProvenanceOrigin::AgentInferred),
        );
        e.domain = Domain::Business;
        e.domains = vec![Domain::Business, Domain::Project];
        assert_eq!(e.all_domains(), vec![Domain::Business, Domain::Project]);
    }

    #[test]
    fn envelope_serde_roundtrip() {
        let mut e = Envelope::new(
            "obj_2",
            "wiki_page",
            now(),
            Provenance::new(ProvenanceOrigin::PavleDirect),
        );
        e.categories = vec!["gtm".into()];
        e.sensitivity = Sensitivity::Confidential;
        let j = serde_json::to_string(&e).unwrap();
        let back: Envelope = serde_json::from_str(&j).unwrap();
        assert_eq!(back.id, "obj_2");
        assert_eq!(back.sensitivity, Sensitivity::Confidential);
        assert!(back.is_tagged());
        // frontmatter key for type is "type"
        assert!(j.contains("\"type\":\"wiki_page\""));
    }
}

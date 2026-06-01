//! Governed domain enum + orthogonal risk tags (RECONCILIATION R1/R3).
//!
//! `domain` is a GOVERNED set (the 9 builtins are closed by default; adding one
//! is review-gated — it mints a `domain_policy`). It is orthogonal to the
//! sensitivity level: exposure filtering checks `domains ⊆ caller_allowed` as a
//! SEPARATE gate condition from the `level <= ceiling` test.
//!
//! `risk_tags` are orthogonal flags that can force a review or raise the level,
//! never themselves a ladder.

use serde::{Deserialize, Serialize};

/// The 9 governed life/work domains (Constitution Law 6). `Other` tolerates
/// unknown values on read (flagged for review), never panics.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Domain {
    #[default]
    Business,
    Personal,
    Project,
    Client,
    Relationship,
    Health,
    Legal,
    Financial,
    Public,
    /// Unknown domain read from storage — flagged for review.
    Other(String),
}

impl Domain {
    /// The 9 governed builtins (excludes `Other`).
    pub fn builtins() -> [Domain; 9] {
        [
            Domain::Business,
            Domain::Personal,
            Domain::Project,
            Domain::Client,
            Domain::Relationship,
            Domain::Health,
            Domain::Legal,
            Domain::Financial,
            Domain::Public,
        ]
    }

    /// High-water domains that default to local-only / no plaintext mirror (R3/§6).
    pub fn is_high_water(&self) -> bool {
        matches!(
            self,
            Domain::Personal
                | Domain::Relationship
                | Domain::Health
                | Domain::Legal
                | Domain::Financial
                | Domain::Client
        )
    }
}

impl std::fmt::Display for Domain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Domain::Business => "business",
            Domain::Personal => "personal",
            Domain::Project => "project",
            Domain::Client => "client",
            Domain::Relationship => "relationship",
            Domain::Health => "health",
            Domain::Legal => "legal",
            Domain::Financial => "financial",
            Domain::Public => "public",
            Domain::Other(s) => s,
        };
        write!(f, "{s}")
    }
}

impl std::str::FromStr for Domain {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Ok(match s {
            "business" => Domain::Business,
            "personal" => Domain::Personal,
            "project" => Domain::Project,
            "client" => Domain::Client,
            "relationship" => Domain::Relationship,
            "health" => Domain::Health,
            "legal" => Domain::Legal,
            "financial" => Domain::Financial,
            "public" => Domain::Public,
            other => Domain::Other(other.to_string()),
        })
    }
}

impl Serialize for Domain {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Domain {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(s.parse().unwrap())
    }
}

/// Orthogonal risk flags (R1). Not a ladder — each can force review or raise level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskTag {
    Financial,
    Health,
    Relationship,
    Legal,
    Credential,
    Identity,
    Minor,
    ThirdPartyPii,
}

impl std::fmt::Display for RiskTag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            RiskTag::Financial => "financial",
            RiskTag::Health => "health",
            RiskTag::Relationship => "relationship",
            RiskTag::Legal => "legal",
            RiskTag::Credential => "credential",
            RiskTag::Identity => "identity",
            RiskTag::Minor => "minor",
            RiskTag::ThirdPartyPii => "third_party_pii",
        };
        write!(f, "{s}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nine_builtins() {
        assert_eq!(Domain::builtins().len(), 9);
    }

    #[test]
    fn high_water_domains() {
        assert!(Domain::Health.is_high_water());
        assert!(Domain::Relationship.is_high_water());
        assert!(!Domain::Business.is_high_water());
        assert!(!Domain::Public.is_high_water());
    }

    #[test]
    fn parse_display_roundtrip_and_other() {
        for d in Domain::builtins() {
            assert_eq!(d.to_string().parse::<Domain>().unwrap(), d);
        }
        assert_eq!(
            "fitness".parse::<Domain>().unwrap(),
            Domain::Other("fitness".into())
        );
    }

    #[test]
    fn serde_roundtrip() {
        let d = Domain::Other("music".into());
        let j = serde_json::to_string(&d).unwrap();
        assert_eq!(serde_json::from_str::<Domain>(&j).unwrap(), d);
    }
}

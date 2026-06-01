//! Sensitivity ladder + orthogonal risk tags (RECONCILIATION R1).
//!
//! ONE canonical 6-level total-ordered ladder is used everywhere:
//!   `public < shareable < internal < confidential < secret < restricted`
//!
//! Ceiling math (`<=`) touches ONLY the level. `domains` (see [`crate::domain`])
//! and `risk_tags` are orthogonal gate conditions, not part of the scalar.
//! `combine()` = `max(level)` (monotone) — sensitivity only ever rises.
//!
//! `Other(String)` keeps the enum forward-compatible: an unknown value parses
//! to `Other` (never panics) and ranks at the TOP (fail-closed — uncertain
//! content is treated as maximally sensitive).

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

/// The canonical 6-level sensitivity ladder (R1). Total-ordered for ceiling math.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Sensitivity {
    /// Publishable (LinkedIn-ready).
    Public,
    /// Fine for external agents/clients, not for public publishing.
    Shareable,
    /// Default — Pavle's working data.
    #[default]
    Internal,
    /// Business-sensitive (deals, finances-as-business).
    Confidential,
    /// Credential-class only — no durable object *body* is ever `secret`
    /// (only `secret_sighting` fingerprints). Kept for total-order completeness.
    Secret,
    /// Personal / relationship / health / legal / financial high-water mark.
    Restricted,
    /// Unknown value read from storage — fail-closed (ranks as max).
    Other(String),
}

impl Sensitivity {
    /// Numeric rank for the total order. `Other` is treated as the ceiling
    /// (fail-closed): unknown ⇒ maximally sensitive.
    pub fn rank(&self) -> u8 {
        match self {
            Self::Public => 0,
            Self::Shareable => 1,
            Self::Internal => 2,
            Self::Confidential => 3,
            Self::Secret => 4,
            Self::Restricted => 5,
            Self::Other(_) => u8::MAX,
        }
    }

    /// Monotone join: the combined sensitivity of composed objects is the
    /// max of their levels. Sensitivity only ever rises under composition (R1).
    pub fn combine(&self, other: &Sensitivity) -> Sensitivity {
        if self.rank() >= other.rank() {
            self.clone()
        } else {
            other.clone()
        }
    }

    /// `true` if `self` is within (≤) the given ceiling. The ONLY ceiling test.
    pub fn within_ceiling(&self, ceiling: &Sensitivity) -> bool {
        self.rank() <= ceiling.rank()
    }
}

impl PartialOrd for Sensitivity {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Sensitivity {
    fn cmp(&self, other: &Self) -> Ordering {
        self.rank().cmp(&other.rank())
    }
}

impl std::fmt::Display for Sensitivity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Public => write!(f, "public"),
            Self::Shareable => write!(f, "shareable"),
            Self::Internal => write!(f, "internal"),
            Self::Confidential => write!(f, "confidential"),
            Self::Secret => write!(f, "secret"),
            Self::Restricted => write!(f, "restricted"),
            Self::Other(s) => write!(f, "{s}"),
        }
    }
}

impl std::str::FromStr for Sensitivity {
    type Err = std::convert::Infallible;

    /// Never fails — unknown values become `Other` (forward-compat, fail-closed).
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Ok(match s {
            "public" => Self::Public,
            "shareable" => Self::Shareable,
            "internal" => Self::Internal,
            "confidential" => Self::Confidential,
            "secret" => Self::Secret,
            "restricted" => Self::Restricted,
            other => Self::Other(other.to_string()),
        })
    }
}

// Serialize/Deserialize via the string form so `Other(String)` round-trips and
// old 4-level values ("public"/"internal"/"confidential"/"secret") still parse.
impl Serialize for Sensitivity {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Sensitivity {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        // FromStr is infallible.
        Ok(s.parse().unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_is_totally_ordered() {
        assert!(Sensitivity::Public < Sensitivity::Shareable);
        assert!(Sensitivity::Shareable < Sensitivity::Internal);
        assert!(Sensitivity::Internal < Sensitivity::Confidential);
        assert!(Sensitivity::Confidential < Sensitivity::Secret);
        assert!(Sensitivity::Secret < Sensitivity::Restricted);
    }

    #[test]
    fn other_is_fail_closed_max() {
        let other = Sensitivity::Other("weird".into());
        assert!(other > Sensitivity::Restricted);
        assert!(!Sensitivity::Other("x".into()).within_ceiling(&Sensitivity::Restricted));
    }

    #[test]
    fn ceiling_test_uses_level() {
        assert!(Sensitivity::Internal.within_ceiling(&Sensitivity::Confidential));
        assert!(!Sensitivity::Restricted.within_ceiling(&Sensitivity::Internal));
    }

    #[test]
    fn combine_is_monotone_max() {
        assert_eq!(
            Sensitivity::Public.combine(&Sensitivity::Restricted),
            Sensitivity::Restricted
        );
        assert_eq!(
            Sensitivity::Confidential.combine(&Sensitivity::Internal),
            Sensitivity::Confidential
        );
    }

    #[test]
    fn back_compat_parse_and_display() {
        for s in ["public", "internal", "confidential", "secret"] {
            assert_eq!(s.parse::<Sensitivity>().unwrap().to_string(), s);
        }
        // new levels
        for s in ["shareable", "restricted"] {
            assert_eq!(s.parse::<Sensitivity>().unwrap().to_string(), s);
        }
    }

    #[test]
    fn serde_roundtrips_including_other() {
        for s in [
            Sensitivity::Public,
            Sensitivity::Restricted,
            Sensitivity::Other("future".into()),
        ] {
            let j = serde_json::to_string(&s).unwrap();
            let back: Sensitivity = serde_json::from_str(&j).unwrap();
            assert_eq!(s, back);
        }
    }
}

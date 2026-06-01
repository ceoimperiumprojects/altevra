//! Capability / trust types for the tool-skill registry (working draft §5,
//! BUILD_TASKS T2.2). The `CapabilityState` machine itself lives in
//! [`crate::status`]; this module adds the cross-agent trust ladder + the
//! honesty `Support` enum.

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

/// Ordered ladder for cross-agent grants (§5.3). A grant never auto-elevates;
/// `execute` (an action capability) is always review-gated (T9).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TrustLevel {
    #[default]
    None,
    Read,
    Propose,
    Render,
    Install,
    Execute,
}

impl TrustLevel {
    pub fn rank(&self) -> u8 {
        match self {
            TrustLevel::None => 0,
            TrustLevel::Read => 1,
            TrustLevel::Propose => 2,
            TrustLevel::Render => 3,
            TrustLevel::Install => 4,
            TrustLevel::Execute => 5,
        }
    }

    /// `install`/`execute` grants are broad → always require review (T9).
    pub fn requires_review(&self) -> bool {
        self.rank() >= TrustLevel::Install.rank()
    }
}

impl PartialOrd for TrustLevel {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for TrustLevel {
    fn cmp(&self, other: &Self) -> Ordering {
        self.rank().cmp(&other.rank())
    }
}

impl std::fmt::Display for TrustLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            TrustLevel::None => "none",
            TrustLevel::Read => "read",
            TrustLevel::Propose => "propose",
            TrustLevel::Render => "render",
            TrustLevel::Install => "install",
            TrustLevel::Execute => "execute",
        };
        write!(f, "{s}")
    }
}

impl std::str::FromStr for TrustLevel {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "read" => TrustLevel::Read,
            "propose" => TrustLevel::Propose,
            "render" => TrustLevel::Render,
            "install" => TrustLevel::Install,
            "execute" => TrustLevel::Execute,
            _ => TrustLevel::None,
        })
    }
}

/// Capability honesty (§5.2.5, Constitution Law 6): `supported` REQUIRES
/// evidence; absent evidence it is `unverified`. No adapter advertises an
/// unproven native surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Support {
    Supported,
    Unsupported,
    Unverified,
    Fallback,
}

impl Support {
    /// `Supported` is only valid with an evidence ref (T7).
    pub fn is_valid_with_evidence(&self, has_evidence: bool) -> bool {
        match self {
            Support::Supported => has_evidence,
            _ => true,
        }
    }
}

impl std::fmt::Display for Support {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Support::Supported => "supported",
            Support::Unsupported => "unsupported",
            Support::Unverified => "unverified",
            Support::Fallback => "fallback",
        };
        write!(f, "{s}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trust_ladder_ordered() {
        assert!(TrustLevel::None < TrustLevel::Read);
        assert!(TrustLevel::Render < TrustLevel::Install);
        assert!(TrustLevel::Install < TrustLevel::Execute);
    }

    #[test]
    fn install_and_execute_require_review() {
        assert!(!TrustLevel::Read.requires_review());
        assert!(!TrustLevel::Render.requires_review());
        assert!(TrustLevel::Install.requires_review());
        assert!(TrustLevel::Execute.requires_review());
    }

    #[test]
    fn supported_needs_evidence() {
        assert!(!Support::Supported.is_valid_with_evidence(false));
        assert!(Support::Supported.is_valid_with_evidence(true));
        assert!(Support::Unverified.is_valid_with_evidence(false));
    }

    #[test]
    fn trust_roundtrip() {
        for t in ["read", "propose", "render", "install", "execute"] {
            assert_eq!(t.parse::<TrustLevel>().unwrap().to_string(), t);
        }
    }
}

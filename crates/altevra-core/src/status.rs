//! The six SEPARATE status families (RECONCILIATION R2).
//!
//! Never overload one enum/column for all workflows. `quarantined` is a
//! [`RedactionStatus`] (a text-scanning result), NOT an [`ObjectStatus`] — that
//! conflation (in the original P0_CONTRACTS sketch) is corrected here.
//!
//! Every family carries `Other(String)` tolerant parse (no panic on unknown).

/// Generates a string enum with an `Other(String)` escape hatch + Display +
/// (infallible) FromStr + serde via the string form. Forward-compatible:
/// unknown values parse to `Other`, never panic.
macro_rules! string_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident => $str:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub enum $name {
            $($variant,)+
            /// Unknown value read from storage — tolerated, flagged for review.
            Other(String),
        }

        impl $name {
            /// All known (non-`Other`) variants, in declared order.
            pub fn known() -> Vec<$name> {
                vec![$($name::$variant),+]
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    $(Self::$variant => write!(f, $str),)+
                    Self::Other(s) => write!(f, "{s}"),
                }
            }
        }

        impl std::str::FromStr for $name {
            type Err = std::convert::Infallible;
            fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
                Ok(match s {
                    $($str => Self::$variant,)+
                    other => Self::Other(other.to_string()),
                })
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&self.to_string())
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                Ok(s.parse().unwrap())
            }
        }
    };
}

string_enum! {
    /// Object lifecycle. `forgotten` IS lifecycle (§2.8 soft-forget);
    /// `quarantined` is NOT here — it's a [`RedactionStatus`].
    ObjectStatus {
        Draft => "draft",
        Active => "active",
        Superseded => "superseded",
        Archived => "archived",
        Forgotten => "forgotten",
        DeletedTombstone => "deleted_tombstone",
    }
}

string_enum! {
    /// Result of the pre-write safety scan (§2). Separate column on text rows.
    RedactionStatus {
        Unscanned => "unscanned",
        Clean => "clean",
        Redacted => "redacted",
        Quarantined => "quarantined",
        Rejected => "rejected",
    }
}

string_enum! {
    /// Review workflow state.
    ReviewStatus {
        NotRequired => "not_required",
        PendingReview => "pending_review",
        Approved => "approved",
        Rejected => "rejected",
        NeedsChanges => "needs_changes",
        Expired => "expired",
    }
}

string_enum! {
    /// Retention/staleness state (§6). DERIVED, not stored, except `archived`.
    LifecycleState {
        Fresh => "fresh",
        DueForReview => "due_for_review",
        Expired => "expired",
        Archived => "archived",
        RetentionDue => "retention_due",
        DeleteDue => "delete_due",
        LegalHold => "legal_hold",
    }
}

string_enum! {
    /// Installed-component capability state (§5). Computed by `verify`, never
    /// asserted by a payload.
    CapabilityState {
        Discovered => "discovered",
        Installed => "installed",
        Current => "current",
        Outdated => "outdated",
        Drifted => "drifted",
        Broken => "broken",
        Disabled => "disabled",
        NeedsReview => "needs_review",
        Missing => "missing",
        Conflicted => "conflicted",
        Unsupported => "unsupported",
    }
}

string_enum! {
    /// Self-improvement proposal lifecycle (§4).
    ProposalStatus {
        Proposed => "proposed",
        Triaged => "triaged",
        Approved => "approved",
        Applied => "applied",
        Rejected => "rejected",
        Superseded => "superseded",
        Withdrawn => "withdrawn",
        Deprecated => "deprecated",
    }
}

impl ObjectStatus {
    /// Statuses included in a default agent-facing read (I3): excludes
    /// superseded/archived/forgotten/deleted.
    pub fn is_default_readable(&self) -> bool {
        matches!(self, ObjectStatus::Draft | ObjectStatus::Active)
    }

    /// Legal lifecycle transitions (§1.5 generic family). An illegal transition
    /// is rejected and opens a review_item (I8).
    pub fn can_transition_to(&self, next: &ObjectStatus) -> bool {
        use ObjectStatus::*;
        matches!(
            (self, next),
            (Draft, Active)
                | (Active, Superseded)
                | (Active, Archived)
                | (Active, Forgotten)
                | (Active, DeletedTombstone)
                | (Superseded, Archived)
                | (Archived, Active) // reopen
                | (Forgotten, Active) // un-forget
                | (Archived, DeletedTombstone)
                | (Superseded, DeletedTombstone)
                | (Forgotten, DeletedTombstone)
        )
    }
}

impl ProposalStatus {
    /// A proposal may only reach `Applied` from `Approved` (Tier ≥1) or directly
    /// (Tier 0). `Applied → Deprecated` when its success metric decays.
    pub fn can_transition_to(&self, next: &ProposalStatus) -> bool {
        use ProposalStatus::*;
        matches!(
            (self, next),
            (Proposed, Triaged)
                | (Proposed, Approved)
                | (Proposed, Applied) // Tier 0 direct
                | (Proposed, Rejected)
                | (Proposed, Withdrawn)
                | (Triaged, Approved)
                | (Triaged, Rejected)
                | (Approved, Applied)
                | (Approved, Rejected)
                | (Applied, Deprecated)
                | (Applied, Superseded)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_status_default_readability() {
        assert!(ObjectStatus::Active.is_default_readable());
        assert!(!ObjectStatus::Superseded.is_default_readable());
        assert!(!ObjectStatus::Forgotten.is_default_readable());
    }

    #[test]
    fn quarantined_is_redaction_not_object_status() {
        // quarantined must parse as a RedactionStatus value...
        assert_eq!(
            "quarantined".parse::<RedactionStatus>().unwrap(),
            RedactionStatus::Quarantined
        );
        // ...and NOT be a known ObjectStatus (it falls through to Other).
        assert_eq!(
            "quarantined".parse::<ObjectStatus>().unwrap(),
            ObjectStatus::Other("quarantined".into())
        );
    }

    #[test]
    fn legal_object_transitions() {
        assert!(ObjectStatus::Draft.can_transition_to(&ObjectStatus::Active));
        assert!(ObjectStatus::Active.can_transition_to(&ObjectStatus::Superseded));
        assert!(!ObjectStatus::Active.can_transition_to(&ObjectStatus::Draft));
    }

    #[test]
    fn proposal_cannot_apply_without_approval_path() {
        // direct Proposed->Applied allowed only for Tier-0 (caller enforces tier)
        assert!(ProposalStatus::Approved.can_transition_to(&ProposalStatus::Applied));
        assert!(!ProposalStatus::Rejected.can_transition_to(&ProposalStatus::Applied));
    }

    #[test]
    fn unknown_values_tolerated_everywhere() {
        assert_eq!(
            "future".parse::<ReviewStatus>().unwrap(),
            ReviewStatus::Other("future".into())
        );
        assert_eq!(
            "novel".parse::<CapabilityState>().unwrap(),
            CapabilityState::Other("novel".into())
        );
    }

    #[test]
    fn serde_roundtrips() {
        let s = ObjectStatus::Forgotten;
        let j = serde_json::to_string(&s).unwrap();
        assert_eq!(j, "\"forgotten\"");
        assert_eq!(serde_json::from_str::<ObjectStatus>(&j).unwrap(), s);
    }

    #[test]
    fn known_lists_have_expected_counts() {
        assert_eq!(ObjectStatus::known().len(), 6);
        assert_eq!(RedactionStatus::known().len(), 5);
        assert_eq!(ReviewStatus::known().len(), 6);
        assert_eq!(LifecycleState::known().len(), 7);
        assert_eq!(CapabilityState::known().len(), 11);
        assert_eq!(ProposalStatus::known().len(), 8);
    }
}

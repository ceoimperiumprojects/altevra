//! Resident runtime contract (working draft §4, RECONCILIATION R10/R14).
//!
//! A *resident mode* is a small, single-purpose agent (MOD-2) — never a monolith.
//! It declares a model ROLE (never a concrete model) and a sensitivity ceiling,
//! reads a scoped context packet, and emits ONE kind of typed, schema-validated,
//! review-routed output. In P0.5 every role resolves to the noop provider, so the
//! whole contract runs with no keys (the "just add API keys" seam).
//!
//! Invariants enforced here (pure, testable):
//!  - **SI-7:** a mode that may touch personal data MUST use the `local_private`
//!    role (personal data never leaves the machine).
//!  - **SI-14:** output that fails schema validation yields `FailedSchema` and the
//!    caller writes NOTHING (dry-run / proposal-only).

use crate::security::Sensitivity;
use serde::{Deserialize, Serialize};

/// A small single-purpose resident agent (MOD-2).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResidentMode {
    pub name: String,
    pub description: String,
    /// Matches `altevra_llm::ModelRole::as_str()` — a role, never a model.
    pub model_role: String,
    pub sensitivity_ceiling: Sensitivity,
    pub personal_data_allowed: bool,
    pub enabled: bool,
}

impl ResidentMode {
    /// SI-7: a mode allowed to touch personal data MUST route to `local_private`.
    pub fn validate_role_ceiling(&self) -> Result<(), String> {
        if self.personal_data_allowed && self.model_role != "local_private" {
            return Err(format!(
                "mode '{}' is personal_data_allowed but role is '{}' — SI-7 requires local_private",
                self.name, self.model_role
            ));
        }
        Ok(())
    }
}

/// One proposal a resident run emits. Review-routed; never auto-applied in dry-run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResidentProposal {
    pub kind: String,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub evidence_refs: Vec<String>,
}

/// The schema-validated output of a resident run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ResidentOutput {
    #[serde(default)]
    pub proposals: Vec<ResidentProposal>,
}

/// Terminal status of a resident run (recorded on the `resident_run`/brain_jobs row).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResidentRunStatus {
    /// Ran and produced schema-valid output (possibly empty).
    Completed,
    /// Provider output failed schema validation — zero writes (SI-14).
    FailedSchema,
    /// Budget/firewall aborted the run before it produced output.
    AbortedBudget,
    /// Skipped before running (e.g. SI-7 contract violation, disabled mode).
    Skipped,
}

impl ResidentRunStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ResidentRunStatus::Completed => "completed",
            ResidentRunStatus::FailedSchema => "failed_schema",
            ResidentRunStatus::AbortedBudget => "aborted_budget",
            ResidentRunStatus::Skipped => "skipped",
        }
    }
}

/// Parse + schema-validate a provider's raw output (SI-14). A real provider that
/// returns non-conforming text yields `Err` → the caller records `FailedSchema`
/// and writes nothing.
pub fn parse_resident_output(raw: &str) -> Result<ResidentOutput, String> {
    serde_json::from_str::<ResidentOutput>(raw.trim())
        .map_err(|e| format!("resident output schema invalid: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(role: &str, personal: bool) -> ResidentMode {
        ResidentMode {
            name: "m".into(),
            description: "d".into(),
            model_role: role.into(),
            sensitivity_ceiling: Sensitivity::Internal,
            personal_data_allowed: personal,
            enabled: true,
        }
    }

    #[test]
    fn si7_personal_requires_local_private() {
        // personal_data_allowed + non-local role → rejected.
        assert!(mode("cheap_worker", true).validate_role_ceiling().is_err());
        assert!(mode("strong_reasoner", true)
            .validate_role_ceiling()
            .is_err());
        // personal + local_private → ok; non-personal any role → ok.
        assert!(mode("local_private", true).validate_role_ceiling().is_ok());
        assert!(mode("cheap_worker", false).validate_role_ceiling().is_ok());
    }

    #[test]
    fn schema_valid_output_parses() {
        let raw = r#"{"proposals":[{"kind":"memory","title":"t","body":"b","evidence_refs":["turn:1"]}]}"#;
        let out = parse_resident_output(raw).unwrap();
        assert_eq!(out.proposals.len(), 1);
        assert_eq!(out.proposals[0].kind, "memory");
        // empty proposal set is also schema-valid.
        assert_eq!(
            parse_resident_output(r#"{"proposals":[]}"#)
                .unwrap()
                .proposals
                .len(),
            0
        );
        assert!(parse_resident_output("{}").unwrap().proposals.is_empty());
    }

    #[test]
    fn schema_invalid_output_rejected() {
        // SI-14: garbage / wrong shape → Err (caller writes nothing).
        assert!(parse_resident_output("not json at all").is_err());
        assert!(parse_resident_output(r#"{"proposals":[{"title":"missing kind"}]}"#).is_err());
        assert!(parse_resident_output(r#"{"proposals":"should be array"}"#).is_err());
    }
}

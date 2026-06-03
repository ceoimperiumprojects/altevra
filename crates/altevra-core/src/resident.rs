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
///
/// Real models frequently wrap their JSON in prose and/or ```json fences. We are
/// tolerant about *envelope extraction* (try the trimmed text, then strip fences,
/// then scan for the first balanced top-level JSON object/array) but STRICT about
/// the schema: only a well-formed generic envelope `{"proposals":[{kind,title,
/// body,evidence_refs?}]}` parses. If no valid generic envelope is found, this
/// returns `Err` and the caller stays `FailedSchema` with zero writes (SI-14).
pub fn parse_resident_output(raw: &str) -> Result<ResidentOutput, String> {
    let trimmed = raw.trim();
    // Fast path: the whole response is the envelope.
    if let Ok(out) = serde_json::from_str::<ResidentOutput>(trimmed) {
        return Ok(out);
    }
    // Tolerant path: extract the first balanced top-level JSON value (object or
    // array) from prose / markdown fences, then validate it against the schema.
    for candidate in extract_json_candidates(trimmed) {
        if let Ok(out) = serde_json::from_str::<ResidentOutput>(&candidate) {
            return Ok(out);
        }
        // A bare array of proposals is also accepted as the generic shape.
        if let Ok(props) = serde_json::from_str::<Vec<ResidentProposal>>(&candidate) {
            return Ok(ResidentOutput { proposals: props });
        }
    }
    Err("resident output schema invalid: no generic {proposals:[...]} envelope found".to_string())
}

/// Yield candidate JSON substrings from a possibly-prose response: each balanced
/// top-level `{...}` or `[...]` region (brace/bracket matched, string-aware). The
/// scanner ignores braces inside JSON strings and respects backslash escapes, so a
/// `}` inside a string body never closes the region early. Candidates are returned
/// in document order so the FIRST valid envelope wins (a common, safe pattern).
fn extract_json_candidates(s: &str) -> Vec<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        let open = bytes[i];
        if open == b'{' || open == b'[' {
            let close = if open == b'{' { b'}' } else { b']' };
            let mut depth = 0i32;
            let mut in_str = false;
            let mut escaped = false;
            let mut j = i;
            let mut end = None;
            while j < bytes.len() {
                let c = bytes[j];
                if in_str {
                    if escaped {
                        escaped = false;
                    } else if c == b'\\' {
                        escaped = true;
                    } else if c == b'"' {
                        in_str = false;
                    }
                } else if c == b'"' {
                    in_str = true;
                } else if c == open {
                    depth += 1;
                } else if c == close {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(j);
                        break;
                    }
                }
                j += 1;
            }
            if let Some(e) = end {
                out.push(s[i..=e].to_string());
                i = e + 1;
                continue;
            }
        }
        i += 1;
    }
    out
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

    #[test]
    fn tolerant_extraction_from_prose_and_fences() {
        // Real models wrap the envelope in prose and/or ```json fences. The
        // tolerant path must still find the generic envelope and validate it.
        let fenced = "Here is the result:\n```json\n{\"proposals\":[{\"kind\":\"insight\",\"title\":\"t\",\"body\":\"b\",\"evidence_refs\":[\"obj:1\"]}]}\n```\nHope that helps!";
        let out = parse_resident_output(fenced).unwrap();
        assert_eq!(out.proposals.len(), 1);
        assert_eq!(out.proposals[0].kind, "insight");
        assert_eq!(out.proposals[0].evidence_refs, vec!["obj:1".to_string()]);

        // Leading prose, no fences.
        let prose = "Sure. {\"proposals\":[{\"kind\":\"memory\",\"title\":\"x\",\"body\":\"y\"}]}";
        assert_eq!(parse_resident_output(prose).unwrap().proposals.len(), 1);

        // A `}` inside a string body must not close the object early.
        let braces_in_body = "noise {\"proposals\":[{\"kind\":\"insight\",\"title\":\"a\",\"body\":\"text with } brace and { brace\"}]} trailing";
        let out = parse_resident_output(braces_in_body).unwrap();
        assert_eq!(out.proposals[0].body, "text with } brace and { brace");

        // A bare array of proposals is also a valid generic shape.
        let bare = "[{\"kind\":\"insight\",\"title\":\"a\",\"body\":\"b\"}]";
        assert_eq!(parse_resident_output(bare).unwrap().proposals.len(), 1);
    }

    #[test]
    fn tolerant_path_keeps_si14_for_pure_prose() {
        // SI-14: a rich prose answer with NO JSON value at all → Err, zero writes.
        // (This is exactly the pre-fix live GPT-5.5 markdown-only output.)
        let markdown = "## Main correlation\n\nAltevra is a context/memory layer.\n\n- bullet one\n- bullet two\n\nSuggested focus: ship v1.";
        assert!(parse_resident_output(markdown).is_err());

        // A JSON object whose proposals carry a wrong shape (missing `kind`) is NOT
        // a valid envelope → Err (zero writes), even though `proposals` is present.
        assert!(
            parse_resident_output("prose {\"proposals\":[{\"title\":\"no kind\"}]} more").is_err()
        );

        // A JSON object that simply lacks `proposals` parses as an EMPTY envelope
        // (proposals defaults to []): schema-valid, zero proposals → zero writes via
        // the Completed path. This is intentional (mirrors `{}` parsing), not a leak.
        let empty = parse_resident_output("note: {\"summary\":\"nope\"} end").unwrap();
        assert!(empty.proposals.is_empty());
    }
}

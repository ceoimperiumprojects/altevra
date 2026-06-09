//! SkillOpt edit engine (PLAN-ALIVE §P3a) — pure Rust, zero LLM.
//!
//! Port of Hivemind's `skill-edits.ts` ALGORITHM (Apache-2.0 teardown in
//! docs/research/hivemind/01-skillify-engine.md) to deterministic, pure,
//! unit-testable Rust:
//!
//!  - **4 bounded edit ops** — `append | insert_after | replace | delete` —
//!    anchored by EXACT substring match. A hallucinated anchor that isn't
//!    present is SKIPPED with a reason; it never corrupts the document and
//!    never panics.
//!  - **Edit budget** ("textual learning rate", default 3) — at most N edits
//!    are APPLIED per pass; a large budget overfits to a single failure, a
//!    small budget nudges the doc. Skipped edits do not consume budget.
//!  - **Protected slow-update region** — `<!-- ALTEVRA_SLOW_UPDATE_START -->`
//!    / `<!-- ALTEVRA_SLOW_UPDATE_END -->` fences longitudinal guidance that
//!    fast per-failure edits must never touch. Any edit whose anchor range
//!    OVERLAPS a protected region (not just starts inside it) is skipped.
//!    Appends land ABOVE the first protected region so fast updates never
//!    push into the slow one. An unclosed START fail-safes: the rest of the
//!    file is protected.
//!  - **Order-independent fingerprint** of an edit set (canonical JSON, sort,
//!    sha256) — the key for the `skillopt_meta` cross-run memory so a tried
//!    edit set is never re-proposed.
//!
//! Grammar note (locked decision PLAN-ALIVE Key Decisions #9): the protected
//! region uses the ALTEVRA_MANAGED marker FAMILY — the SkillOpt slow-update
//! semantics are expressed as `ALTEVRA_SLOW_UPDATE_START/END`; no file ever
//! carries two competing grammars.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::ops::Range;

/// Start of a protected (slow-update) region. Edits may never touch anything
/// from this marker through the matching END marker.
pub const SLOW_UPDATE_START: &str = "<!-- ALTEVRA_SLOW_UPDATE_START -->";
/// End of a protected (slow-update) region.
pub const SLOW_UPDATE_END: &str = "<!-- ALTEVRA_SLOW_UPDATE_END -->";

/// The default "textual learning rate" — at most this many edits are applied
/// per optimizer pass (Hivemind default, `skill-proposer.ts:80`).
pub const DEFAULT_EDIT_BUDGET: usize = 3;

/// One bounded, deterministic edit over a skill body.
///
/// JSON shape (the proposer/CLI wire format):
/// ```json
/// [{"op":"append","text":"..."},
///  {"op":"insert_after","anchor":"## Usage","text":"..."},
///  {"op":"replace","from":"old","to":"new"},
///  {"op":"delete","text":"stale line"}]
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SkillEdit {
    /// Append `text` at the end of the body (or ABOVE the first protected
    /// region, so fast updates never push into the slow region).
    Append { text: String },
    /// Insert `text` on a new line immediately after the first exact
    /// occurrence of `anchor`.
    InsertAfter { anchor: String, text: String },
    /// Replace the first exact occurrence of `from` with `to`.
    Replace { from: String, to: String },
    /// Delete the first exact occurrence of `text`.
    Delete { text: String },
}

impl SkillEdit {
    /// Short op discriminant (for meta summaries / display).
    pub fn op_name(&self) -> &'static str {
        match self {
            SkillEdit::Append { .. } => "append",
            SkillEdit::InsertAfter { .. } => "insert_after",
            SkillEdit::Replace { .. } => "replace",
            SkillEdit::Delete { .. } => "delete",
        }
    }

    /// One-line human summary (truncated) for `skillopt_meta.ops`.
    pub fn summary(&self) -> String {
        fn trunc(s: &str) -> String {
            let t: String = s.chars().take(60).collect();
            if t.len() < s.len() {
                format!("{t}…")
            } else {
                t
            }
        }
        match self {
            SkillEdit::Append { text } => format!("append: {}", trunc(text)),
            SkillEdit::InsertAfter { anchor, text } => {
                format!("insert_after '{}': {}", trunc(anchor), trunc(text))
            }
            SkillEdit::Replace { from, to } => {
                format!("replace '{}' -> '{}'", trunc(from), trunc(to))
            }
            SkillEdit::Delete { text } => format!("delete: {}", trunc(text)),
        }
    }
}

/// Why an edit was skipped instead of applied. Skips are NEVER errors — the
/// engine must survive hallucinated anchors without corrupting the doc.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// The anchor / `from` / `text` substring does not occur in the body.
    AnchorNotFound,
    /// The edit's target range overlaps a protected slow-update region.
    TargetProtected,
    /// The edit budget (textual learning rate) was already spent.
    BudgetExhausted,
    /// The anchor / `from` / `text` is empty — nothing to anchor on.
    EmptyTarget,
}

impl SkipReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            SkipReason::AnchorNotFound => "anchor not found",
            SkipReason::TargetProtected => "target inside protected slow-update region",
            SkipReason::BudgetExhausted => "edit budget exhausted",
            SkipReason::EmptyTarget => "empty anchor/target",
        }
    }
}

/// An edit that was not applied, with the reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedEdit {
    pub edit: SkillEdit,
    pub reason: SkipReason,
}

/// Result of one deterministic edit pass — pure data, no I/O.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditOutcome {
    /// The body after applying the accepted edits (== input when nothing applied).
    pub edited_body: String,
    /// Edits that were applied, in application order.
    pub applied: Vec<SkillEdit>,
    /// Edits that were skipped, each with its reason.
    pub skipped: Vec<SkippedEdit>,
    /// Whether `edited_body` differs from the input body.
    pub changed: bool,
}

/// Byte ranges of every protected slow-update region in `body`, INCLUDING the
/// marker comments themselves (so an edit can't delete a fence).
///
/// Handles multiple regions. **Fail-safe:** an unclosed START protects the
/// rest of the file. An orphan END (no preceding START) is inert text.
pub fn protected_ranges(body: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut cursor = 0usize;
    while let Some(rel_start) = body[cursor..].find(SLOW_UPDATE_START) {
        let start = cursor + rel_start;
        let search_from = start + SLOW_UPDATE_START.len();
        match body[search_from..].find(SLOW_UPDATE_END) {
            Some(rel_end) => {
                let end = search_from + rel_end + SLOW_UPDATE_END.len();
                ranges.push(start..end);
                cursor = end;
            }
            None => {
                // Unclosed region — protect everything to EOF, fail-safe.
                ranges.push(start..body.len());
                break;
            }
        }
    }
    ranges
}

/// Does the byte range `[start, end)` overlap ANY protected region? Overlap —
/// not containment — so an anchor that begins just before a fence and spans
/// into it is still rejected (Hivemind `targetsProtected` semantics).
pub fn targets_protected(ranges: &[Range<usize>], start: usize, end: usize) -> bool {
    ranges.iter().any(|r| start < r.end && r.start < end)
}

/// The budget knob as a standalone pure function (Hivemind `selectEdits`
/// parity): keep at most the first `budget` edits. `apply_edits` enforces the
/// stronger "first N APPLICABLE" semantics — a skipped edit does not waste
/// learning rate there.
pub fn select_edits(edits: &[SkillEdit], budget: usize) -> Vec<SkillEdit> {
    edits.iter().take(budget).cloned().collect()
}

/// Order-independent fingerprint of an edit set: canonicalize each edit to
/// JSON (serde_json object keys are sorted — BTreeMap), sort the strings,
/// join, sha256, hex. Same edits in a different order → same fingerprint.
/// This is the dedup key for `skillopt_meta` (never re-propose a tried set).
pub fn fingerprint_edits(edits: &[SkillEdit]) -> String {
    let mut canon: Vec<String> = edits
        .iter()
        .map(|e| {
            serde_json::to_string(&serde_json::to_value(e).unwrap_or(serde_json::Value::Null))
                .unwrap_or_default()
        })
        .collect();
    canon.sort();
    let mut hasher = Sha256::new();
    for c in &canon {
        hasher.update(c.as_bytes());
        hasher.update(b"\n");
    }
    hex::encode(hasher.finalize())
}

/// Apply `edits` to `body` under `budget`. Pure and deterministic: same
/// inputs → same `EditOutcome`. Never panics; a bad edit is a per-edit SKIP
/// with a reason, never an error for the whole pass. Protected ranges are
/// recomputed after every applied edit (offsets shift).
pub fn apply_edits(body: &str, edits: &[SkillEdit], budget: usize) -> EditOutcome {
    let mut current = body.to_string();
    let mut applied: Vec<SkillEdit> = Vec::new();
    let mut skipped: Vec<SkippedEdit> = Vec::new();

    for edit in edits {
        if applied.len() >= budget {
            skipped.push(SkippedEdit {
                edit: edit.clone(),
                reason: SkipReason::BudgetExhausted,
            });
            continue;
        }
        match apply_one(&current, edit) {
            Ok(next) => {
                current = next;
                applied.push(edit.clone());
            }
            Err(reason) => skipped.push(SkippedEdit {
                edit: edit.clone(),
                reason,
            }),
        }
    }

    let changed = current != body;
    EditOutcome {
        edited_body: current,
        applied,
        skipped,
        changed,
    }
}

/// Apply a single edit. `Err(reason)` means SKIP — the body is untouched.
fn apply_one(body: &str, edit: &SkillEdit) -> Result<String, SkipReason> {
    let ranges = protected_ranges(body);
    match edit {
        SkillEdit::Append { text } => {
            if text.is_empty() {
                return Err(SkipReason::EmptyTarget);
            }
            // Land ABOVE the first protected region (fast updates never push
            // into the slow region); else at end of body.
            match ranges.first() {
                Some(r) => {
                    let at = r.start;
                    let mut out = String::with_capacity(body.len() + text.len() + 2);
                    out.push_str(&body[..at]);
                    if !out.is_empty() && !out.ends_with('\n') {
                        out.push('\n');
                    }
                    out.push_str(text);
                    if !text.ends_with('\n') {
                        out.push('\n');
                    }
                    out.push_str(&body[at..]);
                    Ok(out)
                }
                None => {
                    let mut out = body.to_string();
                    if !out.is_empty() && !out.ends_with('\n') {
                        out.push('\n');
                    }
                    out.push_str(text);
                    if !text.ends_with('\n') {
                        out.push('\n');
                    }
                    Ok(out)
                }
            }
        }
        SkillEdit::InsertAfter { anchor, text } => {
            if anchor.is_empty() {
                return Err(SkipReason::EmptyTarget);
            }
            let idx = body.find(anchor.as_str()).ok_or(SkipReason::AnchorNotFound)?;
            let end = idx + anchor.len();
            if targets_protected(&ranges, idx, end) {
                return Err(SkipReason::TargetProtected);
            }
            let mut out = String::with_capacity(body.len() + text.len() + 1);
            out.push_str(&body[..end]);
            out.push('\n');
            out.push_str(text);
            out.push_str(&body[end..]);
            Ok(out)
        }
        SkillEdit::Replace { from, to } => {
            if from.is_empty() {
                return Err(SkipReason::EmptyTarget);
            }
            let idx = body.find(from.as_str()).ok_or(SkipReason::AnchorNotFound)?;
            if targets_protected(&ranges, idx, idx + from.len()) {
                return Err(SkipReason::TargetProtected);
            }
            let mut out = String::with_capacity(body.len() + to.len());
            out.push_str(&body[..idx]);
            out.push_str(to);
            out.push_str(&body[idx + from.len()..]);
            Ok(out)
        }
        SkillEdit::Delete { text } => {
            if text.is_empty() {
                return Err(SkipReason::EmptyTarget);
            }
            let idx = body.find(text.as_str()).ok_or(SkipReason::AnchorNotFound)?;
            if targets_protected(&ranges, idx, idx + text.len()) {
                return Err(SkipReason::TargetProtected);
            }
            let mut out = String::with_capacity(body.len());
            out.push_str(&body[..idx]);
            out.push_str(&body[idx + text.len()..]);
            Ok(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = "# Skill\n\n## Usage\nrun the thing\n\n## Notes\nold note here\n";

    fn protected_body() -> String {
        format!(
            "# Skill\n\n## Fast\nfast guidance\n\n{SLOW_UPDATE_START}\nnever touch this longitudinal rule\n{SLOW_UPDATE_END}\n"
        )
    }

    // ---------- happy paths ----------

    #[test]
    fn append_happy_path() {
        let out = apply_edits(
            BODY,
            &[SkillEdit::Append {
                text: "## Appendix\nnew tail".into(),
            }],
            DEFAULT_EDIT_BUDGET,
        );
        assert_eq!(out.applied.len(), 1);
        assert!(out.skipped.is_empty());
        assert!(out.changed);
        assert!(out.edited_body.ends_with("## Appendix\nnew tail\n"));
        assert!(out.edited_body.starts_with(BODY));
    }

    #[test]
    fn insert_after_happy_path() {
        let out = apply_edits(
            BODY,
            &[SkillEdit::InsertAfter {
                anchor: "## Usage".into(),
                text: "ALWAYS check args first".into(),
            }],
            DEFAULT_EDIT_BUDGET,
        );
        assert_eq!(out.applied.len(), 1);
        assert!(out
            .edited_body
            .contains("## Usage\nALWAYS check args first\nrun the thing"));
    }

    #[test]
    fn replace_happy_path() {
        let out = apply_edits(
            BODY,
            &[SkillEdit::Replace {
                from: "old note here".into(),
                to: "fresh note".into(),
            }],
            DEFAULT_EDIT_BUDGET,
        );
        assert_eq!(out.applied.len(), 1);
        assert!(out.edited_body.contains("fresh note"));
        assert!(!out.edited_body.contains("old note here"));
    }

    #[test]
    fn delete_happy_path() {
        let out = apply_edits(
            BODY,
            &[SkillEdit::Delete {
                text: "old note here\n".into(),
            }],
            DEFAULT_EDIT_BUDGET,
        );
        assert_eq!(out.applied.len(), 1);
        assert!(!out.edited_body.contains("old note here"));
        assert!(out.changed);
    }

    // ---------- anchor-miss skips (hallucinated anchors never corrupt) ----------

    #[test]
    fn anchor_miss_is_skip_not_error() {
        let edits = vec![
            SkillEdit::InsertAfter {
                anchor: "## Hallucinated Section".into(),
                text: "x".into(),
            },
            SkillEdit::Replace {
                from: "does not exist".into(),
                to: "y".into(),
            },
            SkillEdit::Delete {
                text: "also missing".into(),
            },
        ];
        let out = apply_edits(BODY, &edits, DEFAULT_EDIT_BUDGET);
        assert!(out.applied.is_empty());
        assert_eq!(out.skipped.len(), 3);
        assert!(out
            .skipped
            .iter()
            .all(|s| s.reason == SkipReason::AnchorNotFound));
        assert_eq!(out.edited_body, BODY, "skips never mutate the body");
        assert!(!out.changed);
    }

    #[test]
    fn empty_target_is_skip() {
        let out = apply_edits(
            BODY,
            &[
                SkillEdit::Delete { text: String::new() },
                SkillEdit::Append { text: String::new() },
            ],
            DEFAULT_EDIT_BUDGET,
        );
        assert!(out.applied.is_empty());
        assert!(out
            .skipped
            .iter()
            .all(|s| s.reason == SkipReason::EmptyTarget));
        assert!(!out.changed);
    }

    // ---------- budget (textual learning rate) ----------

    #[test]
    fn budget_caps_applied_edits_five_in_three_applied() {
        let edits: Vec<SkillEdit> = (0..5)
            .map(|i| SkillEdit::Append {
                text: format!("line {i}"),
            })
            .collect();
        let out = apply_edits(BODY, &edits, 3);
        assert_eq!(out.applied.len(), 3, "budget 3 → exactly 3 applied");
        assert_eq!(out.skipped.len(), 2);
        assert!(out
            .skipped
            .iter()
            .all(|s| s.reason == SkipReason::BudgetExhausted));
        assert!(out.edited_body.contains("line 2"));
        assert!(!out.edited_body.contains("line 3"));
    }

    #[test]
    fn skipped_edits_do_not_consume_budget() {
        let edits = vec![
            SkillEdit::Replace {
                from: "missing anchor".into(),
                to: "x".into(),
            },
            SkillEdit::Append { text: "a".into() },
            SkillEdit::Append { text: "b".into() },
        ];
        let out = apply_edits(BODY, &edits, 2);
        assert_eq!(out.applied.len(), 2, "the anchor-miss must not waste budget");
        assert_eq!(out.skipped.len(), 1);
        assert_eq!(out.skipped[0].reason, SkipReason::AnchorNotFound);
    }

    #[test]
    fn select_edits_truncates() {
        let edits: Vec<SkillEdit> = (0..5)
            .map(|i| SkillEdit::Append {
                text: format!("e{i}"),
            })
            .collect();
        assert_eq!(select_edits(&edits, 3).len(), 3);
        assert_eq!(select_edits(&edits, 0).len(), 0);
        assert_eq!(select_edits(&edits, 99).len(), 5);
    }

    // ---------- protected slow-update region ----------

    #[test]
    fn edit_inside_protected_region_is_skipped() {
        let body = protected_body();
        let out = apply_edits(
            &body,
            &[
                SkillEdit::Replace {
                    from: "never touch this longitudinal rule".into(),
                    to: "hacked".into(),
                },
                SkillEdit::Delete {
                    text: "longitudinal rule".into(),
                },
                SkillEdit::InsertAfter {
                    anchor: SLOW_UPDATE_START.into(),
                    text: "smuggled".into(),
                },
            ],
            DEFAULT_EDIT_BUDGET,
        );
        assert!(out.applied.is_empty());
        assert_eq!(out.skipped.len(), 3);
        assert!(out
            .skipped
            .iter()
            .all(|s| s.reason == SkipReason::TargetProtected));
        assert_eq!(out.edited_body, body);
    }

    #[test]
    fn edit_outside_protected_region_applies() {
        let body = protected_body();
        let out = apply_edits(
            &body,
            &[SkillEdit::Replace {
                from: "fast guidance".into(),
                to: "updated fast guidance".into(),
            }],
            DEFAULT_EDIT_BUDGET,
        );
        assert_eq!(out.applied.len(), 1);
        assert!(out.edited_body.contains("updated fast guidance"));
        assert!(out
            .edited_body
            .contains("never touch this longitudinal rule"));
    }

    #[test]
    fn anchor_spanning_into_protected_region_is_skipped() {
        // Anchor starts before the fence and spans into it — must be rejected
        // (overlap semantics, not starts-inside semantics).
        let body = protected_body();
        let spanning = format!("fast guidance\n\n{SLOW_UPDATE_START}");
        assert!(body.contains(&spanning), "fixture sanity");
        let out = apply_edits(
            &body,
            &[SkillEdit::Delete { text: spanning }],
            DEFAULT_EDIT_BUDGET,
        );
        assert!(out.applied.is_empty());
        assert_eq!(out.skipped[0].reason, SkipReason::TargetProtected);
    }

    #[test]
    fn append_lands_above_protected_region() {
        let body = protected_body();
        let out = apply_edits(
            &body,
            &[SkillEdit::Append {
                text: "appended fast update".into(),
            }],
            DEFAULT_EDIT_BUDGET,
        );
        assert_eq!(out.applied.len(), 1);
        let appended_at = out.edited_body.find("appended fast update").unwrap();
        let region_at = out.edited_body.find(SLOW_UPDATE_START).unwrap();
        assert!(
            appended_at < region_at,
            "append must land ABOVE the protected region"
        );
        // Region content intact.
        assert!(out
            .edited_body
            .contains("never touch this longitudinal rule"));
    }

    #[test]
    fn multiple_regions_all_protected() {
        let body = format!(
            "intro\n{SLOW_UPDATE_START}\nrule one\n{SLOW_UPDATE_END}\nmiddle editable\n{SLOW_UPDATE_START}\nrule two\n{SLOW_UPDATE_END}\ntail\n"
        );
        let ranges = protected_ranges(&body);
        assert_eq!(ranges.len(), 2);
        let out = apply_edits(
            &body,
            &[
                SkillEdit::Delete {
                    text: "rule one".into(),
                },
                SkillEdit::Delete {
                    text: "rule two".into(),
                },
                SkillEdit::Replace {
                    from: "middle editable".into(),
                    to: "middle EDITED".into(),
                },
            ],
            DEFAULT_EDIT_BUDGET,
        );
        assert_eq!(out.applied.len(), 1, "only the between-regions edit applies");
        assert_eq!(out.skipped.len(), 2);
        assert!(out.edited_body.contains("middle EDITED"));
        assert!(out.edited_body.contains("rule one"));
        assert!(out.edited_body.contains("rule two"));
    }

    #[test]
    fn unclosed_region_fail_safe_protects_rest_of_file() {
        let body = format!("editable head\n{SLOW_UPDATE_START}\nrule\nmore text to EOF\n");
        let ranges = protected_ranges(&body);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].end, body.len(), "unclosed → protected to EOF");

        let out = apply_edits(
            &body,
            &[
                SkillEdit::Delete {
                    text: "more text to EOF".into(),
                },
                SkillEdit::Replace {
                    from: "editable head".into(),
                    to: "edited head".into(),
                },
            ],
            DEFAULT_EDIT_BUDGET,
        );
        // Inside the unclosed region → skipped; before it → applied.
        assert_eq!(out.skipped.len(), 1);
        assert_eq!(out.skipped[0].reason, SkipReason::TargetProtected);
        assert_eq!(out.applied.len(), 1);
        assert!(out.edited_body.contains("edited head"));
        assert!(out.edited_body.contains("more text to EOF"));
    }

    // ---------- fingerprint ----------

    #[test]
    fn fingerprint_is_order_independent() {
        let a = SkillEdit::Append { text: "x".into() };
        let b = SkillEdit::Replace {
            from: "f".into(),
            to: "t".into(),
        };
        let c = SkillEdit::Delete { text: "d".into() };
        let fp1 = fingerprint_edits(&[a.clone(), b.clone(), c.clone()]);
        let fp2 = fingerprint_edits(&[c, a, b]);
        assert_eq!(fp1, fp2, "same set, different order → same fingerprint");
        assert_eq!(fp1.len(), 64, "sha256 hex");
    }

    #[test]
    fn fingerprint_differs_for_different_sets() {
        let fp1 = fingerprint_edits(&[SkillEdit::Append { text: "x".into() }]);
        let fp2 = fingerprint_edits(&[SkillEdit::Append { text: "y".into() }]);
        let fp3 = fingerprint_edits(&[SkillEdit::Delete { text: "x".into() }]);
        assert_ne!(fp1, fp2);
        assert_ne!(fp1, fp3, "same payload, different op → different fingerprint");
    }

    // ---------- wire format ----------

    #[test]
    fn json_round_trip() {
        let json = r###"[
            {"op":"append","text":"a"},
            {"op":"insert_after","anchor":"## H","text":"b"},
            {"op":"replace","from":"x","to":"y"},
            {"op":"delete","text":"z"}
        ]"###;
        let edits: Vec<SkillEdit> = serde_json::from_str(json).unwrap();
        assert_eq!(edits.len(), 4);
        assert_eq!(edits[0].op_name(), "append");
        assert_eq!(edits[1].op_name(), "insert_after");
        let back = serde_json::to_string(&edits).unwrap();
        let again: Vec<SkillEdit> = serde_json::from_str(&back).unwrap();
        assert_eq!(edits, again);
    }

    #[test]
    fn deterministic_same_inputs_same_outcome() {
        let edits = vec![
            SkillEdit::InsertAfter {
                anchor: "## Usage".into(),
                text: "step".into(),
            },
            SkillEdit::Append { text: "tail".into() },
        ];
        let o1 = apply_edits(BODY, &edits, 3);
        let o2 = apply_edits(BODY, &edits, 3);
        assert_eq!(o1.edited_body, o2.edited_body);
        assert_eq!(o1.applied, o2.applied);
    }
}

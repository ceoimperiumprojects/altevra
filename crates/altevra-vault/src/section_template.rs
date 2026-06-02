//! Section-level templates (Pavle's directive 2026-06-02: *"hoću da svaki dokument
//! prati šablon ČAK I DELOVE u dokumentu … sve lepo da se piše sve"*).
//!
//! Document-level frontmatter (see [`crate::normalize`]) makes a whole file typed
//! and findable; this module makes each `## ` SECTION conform to a per-type
//! contract of **bold-label fields** Pavle actually writes. The contract is
//! calibrated against the REAL `~/Obsidian/Imperium/Memory/*.md`, NOT invented:
//!
//!   * **decision** — `**Odluka:**` (24/31 real sections) + a "why" slot
//!     (`Zašto`/`Šta znači`/`Razlog`/`Why`/…) + an optional "next" slot
//!     (`Pravilo za primenu`/`Sledeći korak`/`Next action`/…).
//!   * **person** — `**Kontekst:**` + a "role/status" slot (`Uloga`/`Status`) +
//!     an optional "relevance" slot.
//!   * **learning** — FREEFORM (only 4/16 real sections carry a label); a learning
//!     section conforms iff it has a non-empty body. Recognized labels
//!     (`Lekcija`/`Insight`/`Primena`/`Fix`/…) are surfaced but never required.
//!   * **daily / note** — FREEFORM (non-empty body only).
//!
//! A "label" may appear block-level (`**Odluka:** …`) or as a list item
//! (`- **Odluka:** …`) — both styles occur in Pavle's vault, so matching tolerates
//! a leading `-`/`*`/whitespace. A label "satisfied" means present AND followed by
//! some non-empty value on the same line OR the lines beneath it.

use crate::sections::Section;

/// One required/optional field of a section contract. `synonyms` is the set of
/// interchangeable bold labels (without the surrounding `**`/`:`); any one present
/// satisfies the slot. The first synonym is the CANONICAL label used by the
/// scaffolder.
#[derive(Debug, Clone)]
pub struct LabelSlot {
    /// Interchangeable labels (lowercased-compared, but stored display-cased).
    pub synonyms: &'static [&'static str],
    /// `true` → the slot must be satisfied for conformance; `false` → optional
    /// (surfaced in a report but never blocks conformance / scaffolding).
    pub required: bool,
}

impl LabelSlot {
    /// The canonical label (first synonym) — what the scaffolder emits.
    pub fn canonical(&self) -> &'static str {
        self.synonyms[0]
    }
}

/// The per-type section contract.
#[derive(Debug, Clone)]
pub struct SectionContract {
    pub object_type: &'static str,
    /// Ordered label slots (scaffold emits them in this order).
    pub slots: &'static [LabelSlot],
    /// `true` → freeform: a non-empty body is the only requirement (labels, if
    /// any, are optional). `learning`/`daily`/`note` are freeform.
    pub freeform: bool,
}

/// The result of checking one section against its type contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionConformance {
    pub conformant: bool,
    /// Canonical labels for REQUIRED slots that are missing.
    pub missing_labels: Vec<String>,
    /// Recognized optional labels that are present (informational).
    pub present_optional: Vec<String>,
    /// `true` when the section body is effectively empty (no prose at all).
    pub empty: bool,
}

/// Return the section contract for an object type (decision/person/learning/…).
/// Unknown types fall back to the freeform `note` contract — never panics.
pub fn contract_for(object_type: &str) -> SectionContract {
    match object_type {
        "decision" => SectionContract {
            object_type: "decision",
            slots: &[
                LabelSlot {
                    synonyms: &["Odluka", "Decision"],
                    required: true,
                },
                LabelSlot {
                    // the "why / what it means" slot — calibrated synonym set.
                    synonyms: &[
                        "Zašto",
                        "Šta znači",
                        "Razlog",
                        "Why",
                        "Šta to znači u praksi",
                        "Filozofija",
                    ],
                    required: true,
                },
                LabelSlot {
                    // the "next / how-to-apply" slot — optional (many real
                    // sections omit it).
                    synonyms: &[
                        "Pravilo za primenu",
                        "Pravilo",
                        "Sledeći korak",
                        "Sledeci korak",
                        "Next action",
                        "Operating model",
                        "How to apply",
                    ],
                    required: false,
                },
            ],
            freeform: false,
        },
        "person" => SectionContract {
            object_type: "person",
            slots: &[
                LabelSlot {
                    synonyms: &["Kontekst", "Context"],
                    required: true,
                },
                LabelSlot {
                    // role/status slot — calibrated (Uloga 6×, Status 3×).
                    synonyms: &["Uloga", "Status", "Commitment", "Obećanje"],
                    required: true,
                },
                LabelSlot {
                    synonyms: &["Relevance", "Relevantnost", "Fokus", "Tema"],
                    required: false,
                },
            ],
            freeform: false,
        },
        "learning" => SectionContract {
            object_type: "learning",
            // FREEFORM: real Learnings.md sections are mostly plain prose.
            slots: &[
                LabelSlot {
                    synonyms: &["Lekcija", "Learning", "Insight"],
                    required: false,
                },
                LabelSlot {
                    synonyms: &["Primena", "Fix", "Preporuka", "Preventivno"],
                    required: false,
                },
            ],
            freeform: true,
        },
        "daily_brief" | "daily" => SectionContract {
            object_type: "daily_brief",
            slots: &[],
            freeform: true,
        },
        _ => SectionContract {
            object_type: "note",
            slots: &[],
            freeform: true,
        },
    }
}

/// Strip a SINGLE leading list marker (`- ` / `* ` / `+ `) if present, else the
/// line unchanged. Critically does NOT use `trim_start_matches`, which would eat
/// the `**` bold markers (since `*` is a list marker char).
fn strip_list_marker(line: &str) -> &str {
    let t = line.trim_start();
    for m in ["- ", "* ", "+ "] {
        if let Some(rest) = t.strip_prefix(m) {
            return rest.trim_start();
        }
    }
    t
}

/// Does `body` contain the bold label `label`, satisfied (followed by a value)?
/// Matches both block-level (`**Odluka:** …`) and list-item (`- **Odluka:** …`)
/// styles, case-insensitively on the label text.
fn label_satisfied(body: &str, label: &str) -> bool {
    let want = label.to_lowercase();
    let lines: Vec<&str> = body.lines().collect();
    for (i, raw) in lines.iter().enumerate() {
        // Strip a leading list marker, then look for the `**Label:**` token.
        let trimmed = strip_list_marker(raw);
        let Some(rest) = trimmed.strip_prefix("**") else {
            continue;
        };
        // rest = `Label:** value`  OR  `Label**: value` (tolerate both).
        let (found_label, after) = if let Some(idx) = rest.find(":**") {
            (&rest[..idx], &rest[idx + 3..])
        } else if let Some(idx) = rest.find("**") {
            // `**Label**:` style
            let lbl = &rest[..idx];
            let tail = rest[idx + 2..].trim_start();
            (lbl, tail.strip_prefix(':').unwrap_or(tail))
        } else {
            continue;
        };
        if found_label.trim().to_lowercase() != want {
            continue;
        }
        // Satisfied if there's a value on this line …
        if !after.trim().is_empty() {
            return true;
        }
        // … or a following PROSE line (not blank, not another bold-label line —
        // a sibling label is not THIS label's value, and a bare stub is empty).
        for l in lines.iter().skip(i + 1) {
            if l.trim().is_empty() {
                continue;
            }
            // Reached the next bold-label line before any prose → not satisfied.
            if starts_with_bold_label(l) {
                break;
            }
            return true;
        }
        // A label with NO value is NOT satisfied (it's a scaffold stub).
        return false;
    }
    false
}

/// `true` if a line begins a bold label (`**Foo:**` or `**Foo**:`), possibly after
/// a list marker — i.e. it is a field header, not free prose. Used to stop a
/// label's value-scan at the NEXT label (a sibling label is never this label's
/// value, whether it carries a value or is a bare scaffold stub).
fn starts_with_bold_label(line: &str) -> bool {
    let t = strip_list_marker(line);
    let Some(rest) = t.strip_prefix("**") else {
        return false;
    };
    rest.contains(":**") || rest.contains("**")
}

/// Find which synonym of a slot (if any) is present in the body.
fn slot_present_label(body: &str, slot: &LabelSlot) -> Option<&'static str> {
    slot.synonyms
        .iter()
        .copied()
        .find(|syn| label_satisfied(body, syn))
}

/// Check a section against its type contract.
pub fn section_conformance(section: &Section, object_type: &str) -> SectionConformance {
    let contract = contract_for(object_type);
    let empty = section.body.trim().is_empty();

    if contract.freeform {
        // Freeform: conformant iff non-empty. Surface any recognized labels.
        let present_optional: Vec<String> = contract
            .slots
            .iter()
            .filter_map(|s| slot_present_label(&section.body, s).map(|l| l.to_string()))
            .collect();
        return SectionConformance {
            conformant: !empty,
            missing_labels: Vec::new(),
            present_optional,
            empty,
        };
    }

    let mut missing_labels = Vec::new();
    let mut present_optional = Vec::new();
    for slot in contract.slots {
        match slot_present_label(&section.body, slot) {
            Some(lbl) => {
                if !slot.required {
                    present_optional.push(lbl.to_string());
                }
            }
            None => {
                if slot.required {
                    missing_labels.push(slot.canonical().to_string());
                }
            }
        }
    }
    SectionConformance {
        conformant: !empty && missing_labels.is_empty(),
        missing_labels,
        present_optional,
        empty,
    }
}

/// The empty section skeleton for a NEW section of `object_type` — the canonical
/// required (and optional) labels with blank values, for Pavle to fill in. Daily/
/// freeform types get a single prompting line.
pub fn scaffold_section(object_type: &str) -> String {
    let contract = contract_for(object_type);
    // Freeform types (learning/daily/note) — and any type with no required slots —
    // get a single prompting line, never a label skeleton.
    let has_required = contract.slots.iter().any(|s| s.required);
    if contract.freeform || !has_required {
        return "_(write the note here)_\n".to_string();
    }
    let mut out = String::new();
    for slot in contract.slots {
        let marker = if slot.required { "" } else { " _(optional)_" };
        out.push_str(&format!("**{}:** {marker}\n\n", slot.canonical()));
    }
    out.trim_end().to_string() + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn sec(heading: &str, body: &str) -> Section {
        Section {
            heading: heading.to_string(),
            level: 2,
            body: body.to_string(),
            date: None,
        }
    }

    fn dated(heading: &str, body: &str, d: NaiveDate) -> Section {
        Section {
            heading: heading.to_string(),
            level: 2,
            body: body.to_string(),
            date: Some(d),
        }
    }

    // --- decision contract (calibrated to real Decisions.md) ---

    #[test]
    fn real_decision_section_conforms() {
        // Verbatim shape of the real "Split agent lanes" section.
        let body = "**Odluka:** Ne praviti jednog subagenta.\n\n\
                    **Zašto:** Altevra je strateški bitna ali ReVesta ima market signal.\n\n\
                    **Operating model:** Hermes je command center.";
        let c = section_conformance(&sec("Split agent lanes", body), "decision");
        assert!(c.conformant, "missing: {:?}", c.missing_labels);
        assert!(c.missing_labels.is_empty());
        // Operating model is an optional "next" synonym → surfaced.
        assert!(c.present_optional.iter().any(|l| l == "Operating model"));
    }

    #[test]
    fn decision_with_sta_znaci_why_synonym_conforms() {
        // The first real section uses "Šta znači" + "Pravilo za primenu".
        let body = "**Odluka:** ReVesta hipoteza validirana.\n\n\
                    **Šta znači:** Ne vraćati se u build mode.\n\n\
                    **Pravilo za primenu:** Sledeći rad = calls/outreach.";
        let c = section_conformance(&sec("ReVesta validated", body), "decision");
        assert!(c.conformant);
    }

    #[test]
    fn decision_missing_why_is_nonconformant() {
        let body = "**Odluka:** Samo odluka, bez objašnjenja.";
        let c = section_conformance(&sec("x", body), "decision");
        assert!(!c.conformant);
        // the "why" slot's canonical label is "Zašto".
        assert_eq!(c.missing_labels, vec!["Zašto".to_string()]);
    }

    #[test]
    fn decision_missing_odluka_reports_it() {
        let body = "**Zašto:** objašnjenje bez odluke.";
        let c = section_conformance(&sec("x", body), "decision");
        assert!(!c.conformant);
        assert!(c.missing_labels.contains(&"Odluka".to_string()));
    }

    #[test]
    fn bare_label_with_no_value_is_not_satisfied() {
        // A scaffold stub: labels present but empty → NOT conformant.
        let body = "**Odluka:**\n\n**Zašto:**";
        let c = section_conformance(&sec("x", body), "decision");
        assert!(!c.conformant);
        assert_eq!(c.missing_labels.len(), 2);
    }

    // --- person contract (list-item bold labels, real People.md style) ---

    #[test]
    fn real_person_section_with_list_labels_conforms() {
        // Verbatim shape of the real "Luka" entry: `- **Kontekst:** …`.
        let body = "- **Kontekst:** Telegram thread update 2026-05-18.\n\
                    - **Commitment:** Luka pomaže sa landing page.\n\
                    - **Relevance:** Podržava ReVesta GTM.";
        let c = section_conformance(&sec("Luka — ReVesta landing", body), "person");
        assert!(c.conformant, "missing: {:?}", c.missing_labels);
    }

    #[test]
    fn person_with_uloga_role_synonym_conforms() {
        let body = "- **Kontekst:** Tagovan u thread-u.\n- **Uloga:** TBD.";
        let c = section_conformance(&sec("Ivan", body), "person");
        assert!(c.conformant);
    }

    #[test]
    fn person_missing_role_status_is_nonconformant() {
        let body = "- **Kontekst:** samo kontekst.";
        let c = section_conformance(&sec("x", body), "person");
        assert!(!c.conformant);
        assert_eq!(c.missing_labels, vec!["Uloga".to_string()]);
    }

    // --- learning / daily are freeform ---

    #[test]
    fn real_freeform_learning_conforms_on_nonempty() {
        // Real Learnings.md sections are plain prose with NO labels.
        let body = "LinkedIn ne dozvoljava raw company-page post u personal Featured. Fix: repost.";
        let c = section_conformance(
            &dated(
                "2026-05-21 — LinkedIn",
                body,
                NaiveDate::from_ymd_opt(2026, 5, 21).unwrap(),
            ),
            "learning",
        );
        assert!(c.conformant, "freeform learning conforms when non-empty");
        assert!(c.missing_labels.is_empty());
    }

    #[test]
    fn empty_freeform_section_is_nonconformant() {
        let c = section_conformance(&sec("Empty", "   "), "learning");
        assert!(!c.conformant);
        assert!(c.empty);
    }

    #[test]
    fn daily_and_note_are_freeform() {
        let c = section_conformance(&sec("Session log", "did some work"), "daily_brief");
        assert!(c.conformant);
        let c2 = section_conformance(&sec("Misc", "a note"), "note");
        assert!(c2.conformant);
    }

    // --- scaffolding ---

    #[test]
    fn scaffold_decision_has_canonical_labels() {
        let s = scaffold_section("decision");
        assert!(s.contains("**Odluka:**"));
        assert!(s.contains("**Zašto:**"));
        // optional slot marked
        assert!(s.contains("_(optional)_"));
    }

    #[test]
    fn scaffold_person_has_kontekst_and_uloga() {
        let s = scaffold_section("person");
        assert!(s.contains("**Kontekst:**"));
        assert!(s.contains("**Uloga:**"));
    }

    #[test]
    fn scaffold_freeform_is_a_prompt_line() {
        let s = scaffold_section("learning");
        assert!(s.contains("write the note"));
    }

    #[test]
    fn scaffold_output_is_itself_a_bare_stub_nonconformant() {
        // A freshly scaffolded decision body must NOT pass conformance (it's empty
        // labels) — that's the whole point: it flags "needs Pavle to fill in".
        let s = scaffold_section("decision");
        let c = section_conformance(&sec("new", &s), "decision");
        assert!(!c.conformant);
    }
}

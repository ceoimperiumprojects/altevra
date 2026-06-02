//! Atomization: split a "living" markdown aggregate (Decisions.md, Learnings.md,
//! People.md, a Daily note, …) into its constituent `## ` (level-2) sections, so
//! each section can become its OWN durable object (decision / learning / person /
//! note). This is the heart of Pavle's "Atomizacija" directive: the human writes
//! few files; the machine sees many atomic, individually-recallable objects.
//!
//! Pure + I/O-free + deterministic — the parser is the testable substrate; the
//! capture path (`altevra capture --atomize`) consumes [`parse_sections`] and runs
//! every section body through the same `guard_text` safety gate as whole-file
//! capture.
//!
//! Rules (locked):
//!   * Only `## ` (exactly level-2) headings start a section. `#`/`###`/deeper are
//!     NOT section boundaries — a `### sub-heading` stays inside its parent `##`.
//!   * Text before the first `## ` (the `# Title` + any preamble) is the document
//!     PREAMBLE, never a section.
//!   * A section's body is everything after its heading line up to (but excluding)
//!     the next `## ` heading.
//!   * A `## ` heading with an empty body is skipped — no empty objects.
//!   * `date`: the first `YYYY-MM-DD` found anywhere in the heading text (e.g.
//!     `## 2026-06-02 — ReVesta validated`), else `None`.

use chrono::NaiveDate;
use regex::Regex;
use std::sync::OnceLock;

/// One atomized `## ` section of a markdown aggregate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// The heading text with the leading `## ` and surrounding whitespace stripped,
    /// e.g. `"2026-06-02 — ReVesta direct-call hypothesis validated"`.
    pub heading: String,
    /// Heading level (always `2` for sections we emit — kept for forward-compat).
    pub level: u8,
    /// The section body: every line after the heading up to the next `## `,
    /// trimmed of leading/trailing blank lines. Never empty (empty → skipped).
    pub body: String,
    /// First `YYYY-MM-DD` found in the heading, if any.
    pub date: Option<NaiveDate>,
}

/// Lazily-compiled `YYYY-MM-DD` matcher (no per-call recompile).
fn date_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\b(\d{4})-(\d{2})-(\d{2})\b").expect("valid date regex"))
}

/// Extract the first parseable `YYYY-MM-DD` date appearing anywhere in `heading`.
/// A syntactically-shaped-but-invalid date (e.g. `2026-13-40`) yields `None`.
fn date_in_heading(heading: &str) -> Option<NaiveDate> {
    for caps in date_re().captures_iter(heading) {
        let y: i32 = caps[1].parse().ok()?;
        let m: u32 = caps[2].parse().ok()?;
        let d: u32 = caps[3].parse().ok()?;
        if let Some(date) = NaiveDate::from_ymd_opt(y, m, d) {
            return Some(date);
        }
    }
    None
}

/// Parse a markdown aggregate into its `## ` sections (see module docs for rules).
pub fn parse_sections(content: &str) -> Vec<Section> {
    let mut out = Vec::new();
    let mut current: Option<(String, Vec<&str>)> = None;

    let flush = |out: &mut Vec<Section>, heading: String, body_lines: &[&str]| {
        let body = trim_blank_edges(body_lines);
        if body.is_empty() {
            return; // skip empty-body sections — no empty objects
        }
        let date = date_in_heading(&heading);
        out.push(Section {
            heading,
            level: 2,
            body,
            date,
        });
    };

    for line in content.lines() {
        if let Some(rest) = level2_heading_text(line) {
            // Close the previous section before opening a new one.
            if let Some((heading, body_lines)) = current.take() {
                flush(&mut out, heading, &body_lines);
            }
            current = Some((rest.to_string(), Vec::new()));
        } else if let Some((_, body_lines)) = current.as_mut() {
            body_lines.push(line);
        }
        // else: preamble (before the first `## `) — dropped.
    }
    if let Some((heading, body_lines)) = current.take() {
        flush(&mut out, heading, &body_lines);
    }
    out
}

/// If `line` is a level-2 heading, return its trimmed heading text. `#`, `###`,
/// `####`… are deliberately NOT boundaries — a level-2 section owns everything
/// beneath it including its `###` sub-headings. No leading-whitespace tolerance:
/// a real `## ` heading is at column 0 in these aggregates; an indented `  ## `
/// is code/quoted content, not a section break. A bare `##` with no text returns
/// `Some("")` so the section opens but the empty-body rule still applies.
fn level2_heading_text(line: &str) -> Option<&str> {
    if let Some(rest) = line.strip_prefix("## ") {
        Some(rest.trim())
    } else if line.trim_end() == "##" {
        Some("")
    } else {
        None
    }
}

/// Join body lines and trim leading/trailing blank lines (keeps interior blanks).
fn trim_blank_edges(lines: &[&str]) -> String {
    let start = lines.iter().position(|l| !l.trim().is_empty());
    let end = lines.iter().rposition(|l| !l.trim().is_empty());
    match (start, end) {
        (Some(s), Some(e)) => lines[s..=e].join("\n"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Decisions.md-shaped fixture: a `# Title` preamble + 3 `## ` sections.
    /// Section 1 carries a date; section 3 carries a credential to confirm the
    /// downstream guard redacts it (the parser keeps the raw body; redaction is the
    /// capture layer's job — verified in capture's integration test).
    fn decisions_fixture() -> String {
        let fake = concat!("sk-", "live", "ABCDEFGHIJKLMNOPQRSTUVWX0123");
        format!(
            "# Decisions\n\
             \n\
             Some preamble prose that is NOT a section.\n\
             \n\
             ## 2026-06-02 — ReVesta validated\n\
             \n\
             We will target Florida surplus buyers.\n\
             \n\
             ### sub-detail\n\
             nested stays inside this section\n\
             \n\
             ## Split agent lanes\n\
             \n\
             Build agent and GTM agent are separate.\n\
             \n\
             ## Credential leak section\n\
             \n\
             Here is a key={fake} that must be redacted downstream.\n"
        )
    }

    #[test]
    fn parses_three_sections_skips_preamble() {
        let secs = parse_sections(&decisions_fixture());
        assert_eq!(secs.len(), 3, "exactly 3 ## sections; preamble excluded");
        assert_eq!(secs[0].heading, "2026-06-02 — ReVesta validated");
        assert_eq!(secs[1].heading, "Split agent lanes");
        assert_eq!(secs[2].heading, "Credential leak section");
    }

    #[test]
    fn dated_heading_parses_date_others_none() {
        let secs = parse_sections(&decisions_fixture());
        assert_eq!(secs[0].date, NaiveDate::from_ymd_opt(2026, 6, 2));
        assert_eq!(secs[1].date, None);
        assert_eq!(secs[2].date, None);
    }

    #[test]
    fn nested_subheading_stays_in_parent_section() {
        let secs = parse_sections(&decisions_fixture());
        assert!(
            secs[0].body.contains("### sub-detail"),
            "level-3 heading must remain inside its level-2 parent"
        );
        assert!(secs[0].body.contains("nested stays inside this section"));
        // and must NOT leak into the next section
        assert!(!secs[1].body.contains("sub-detail"));
    }

    #[test]
    fn credential_text_is_preserved_raw_for_downstream_guard() {
        // Parser is pure: it does NOT redact. The fake key survives so the capture
        // layer's guard_text can reject/redact it (proven in capture tests).
        let secs = parse_sections(&decisions_fixture());
        assert!(secs[2].body.contains("sk-live"));
    }

    #[test]
    fn body_trims_blank_edges_keeps_interior() {
        let secs = parse_sections(&decisions_fixture());
        // first body line is content, last is content (no leading/trailing blank)
        assert!(secs[1].body.starts_with("Build agent"));
        assert!(secs[1].body.ends_with("separate."));
    }

    #[test]
    fn empty_body_section_is_skipped() {
        let md = "# T\n\n## Has body\nx\n\n## Empty next\n\n## Also content\ny\n";
        let secs = parse_sections(md);
        let headings: Vec<&str> = secs.iter().map(|s| s.heading.as_str()).collect();
        assert_eq!(headings, vec!["Has body", "Also content"]);
    }

    #[test]
    fn no_sections_when_only_preamble() {
        let md = "# Title only\n\njust prose, no level-2 headings\n";
        assert!(parse_sections(md).is_empty());
    }

    #[test]
    fn level3_alone_is_not_a_section_boundary() {
        let md = "# T\n\n### only level 3\nbody\n";
        // No `## ` at all → no sections.
        assert!(parse_sections(md).is_empty());
    }

    #[test]
    fn invalid_calendar_date_in_heading_yields_none() {
        let md = "# T\n\n## 2026-13-40 not a real date\nbody\n";
        let secs = parse_sections(md);
        assert_eq!(secs.len(), 1);
        assert_eq!(secs[0].date, None);
    }

    #[test]
    fn date_anywhere_in_heading_is_found() {
        let md = "# T\n\n## ReVesta math from 2026-05-26 sprint\nbody\n";
        let secs = parse_sections(md);
        assert_eq!(secs[0].date, NaiveDate::from_ymd_opt(2026, 5, 26));
    }

    #[test]
    fn crlf_aggregate_splits_correctly() {
        let md = "# T\r\n\r\n## One\r\nbody one\r\n\r\n## Two\r\nbody two\r\n";
        let secs = parse_sections(md);
        assert_eq!(secs.len(), 2);
        assert_eq!(secs[0].heading, "One");
        assert!(secs[0].body.contains("body one"));
        assert!(!secs[0].body.contains("body two"));
    }
}

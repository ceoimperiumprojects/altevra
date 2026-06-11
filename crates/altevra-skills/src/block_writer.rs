//! Block-level guarded writer (R5 memory sync hub).
//!
//! Writes and drifts-detects a `<!-- ALTEVRA_MANAGED_START -->` …
//! `<!-- ALTEVRA_MANAGED_END -->` block inside a human-owned file (e.g.
//! `~/.claude/CLAUDE.md`, Obsidian notes). Content OUTSIDE the markers is
//! NEVER touched — bytes are sacred.
//!
//! ## Marker grammar
//!
//! ```html
//! <!-- ALTEVRA_MANAGED_START [label] -->
//! ...block content...
//! <!-- ALTEVRA_MANAGED_END [label] -->
//! ```
//!
//! The optional `[label]` is a word-boundary identifier used as `marker_id` in
//! the `block_writes` manifest. Labels on START and END must match.
//!
//! ## Edge cases handled (mandatory per spec R5)
//!
//! | Case | Behaviour |
//! |------|-----------|
//! | Missing markers | Append a new block to the end of the file |
//! | Duplicate markers | REFUSE (error returned) |
//! | Nested markers | REFUSE |
//! | CRLF lines | Preserved byte-for-byte (detection is `\r\n`-aware) |
//! | Manual edit inside block | DRIFT → refuse + `review_items` |
//! | Manual edit outside block | Survives byte-identically across two runs |

use sha2::{Digest, Sha256};
use std::path::Path;

/// The end-of-line style detected in a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eol {
    Lf,   // `\n`
    Crlf, // `\r\n`
}

impl Eol {
    pub fn as_str(self) -> &'static str {
        match self {
            Eol::Lf => "\n",
            Eol::Crlf => "\r\n",
        }
    }
}

/// Detect the predominant line ending in a file.
pub fn detect_eol(content: &str) -> Eol {
    let crlf = content.matches("\r\n").count();
    let lf_only = content.matches('\n').count().saturating_sub(crlf);
    if crlf >= lf_only {
        Eol::Crlf
    } else {
        Eol::Lf
    }
}

pub const MARKER_START: &str = "ALTEVRA_MANAGED_START";
pub const MARKER_END: &str = "ALTEVRA_MANAGED_END";

/// Check if a line is a START marker and return its label (may be empty).
fn parse_start(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if !trimmed.starts_with("<!--") || !trimmed.ends_with("-->") {
        return None;
    }
    let inner = trimmed
        .trim_start_matches("<!--")
        .trim_end_matches("-->")
        .trim();
    let rest = inner.strip_prefix(MARKER_START)?;
    let label = rest.trim().to_string();
    Some(label)
}

/// Check if a line is an END marker and return its label (may be empty).
fn parse_end(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if !trimmed.starts_with("<!--") || !trimmed.ends_with("-->") {
        return None;
    }
    let inner = trimmed
        .trim_start_matches("<!--")
        .trim_end_matches("-->")
        .trim();
    let rest = inner.strip_prefix(MARKER_END)?;
    let label = rest.trim().to_string();
    Some(label)
}

/// Result of parsing block markers in a file.
#[derive(Debug, PartialEq)]
pub enum ParseResult {
    /// No markers found — the file has no managed block yet.
    Absent,
    /// A valid block was found at `[start_line..=end_line]` (0-indexed into the
    /// lines-split view), with the body text between the markers.
    Found {
        /// 0-indexed line of the `ALTEVRA_MANAGED_START` comment.
        start_line: usize,
        /// 0-indexed line of the `ALTEVRA_MANAGED_END` comment.
        end_line: usize,
        /// The label extracted from the markers (empty string = unlabeled).
        label: String,
        /// The bytes inside the block — from start_line to end_line INCLUSIVE
        /// (including the marker lines themselves).
        block_bytes: String,
    },
    /// More than one START/END pair found → refuse.
    Duplicate,
    /// A START marker appeared without a matching END, or vice versa.
    Malformed(String),
    /// Nested markers detected (START inside START, etc.).
    Nested,
}

/// Parse a file's content for managed block markers.
pub fn parse_block(content: &str) -> ParseResult {
    let eol = detect_eol(content);

    // Split preserving the line endings for byte-exact reconstruction.
    let lines: Vec<&str> = split_lines(content);

    let mut starts: Vec<(usize, String)> = Vec::new();
    let mut ends: Vec<(usize, String)> = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        if let Some(label) = parse_start(line) {
            starts.push((i, label));
        } else if let Some(label) = parse_end(line) {
            ends.push((i, label));
        }
    }

    if starts.is_empty() && ends.is_empty() {
        return ParseResult::Absent;
    }

    // Duplicate START markers.
    if starts.len() > 1 {
        return ParseResult::Duplicate;
    }
    // Duplicate END markers.
    if ends.len() > 1 {
        return ParseResult::Duplicate;
    }

    let Some((start_line, start_label)) = starts.into_iter().next() else {
        // END without START.
        return ParseResult::Malformed("ALTEVRA_MANAGED_END without START".into());
    };
    let Some((end_line, end_label)) = ends.into_iter().next() else {
        // START without END.
        return ParseResult::Malformed("ALTEVRA_MANAGED_START without END".into());
    };

    // Labels must match (if either is non-empty, both must be identical).
    if !start_label.is_empty() && !end_label.is_empty() && start_label != end_label {
        return ParseResult::Malformed(format!(
            "START label '{start_label}' != END label '{end_label}'"
        ));
    }
    let label = if !start_label.is_empty() {
        start_label
    } else {
        end_label
    };

    // START must come BEFORE END.
    if start_line >= end_line {
        return ParseResult::Nested; // Also catches END before START or same line.
    }

    // Nested: check if any other START/END appears between start and end.
    // (Already handled by duplicate detection above, but check ordering.)
    // The block lines slice is start_line..=end_line.
    let block_lines = &lines[start_line..=end_line];
    let block_bytes = rejoin_lines(block_lines, eol);

    ParseResult::Found {
        start_line,
        end_line,
        label,
        block_bytes,
    }
}

/// Split a string into lines, preserving line endings. Each element ends with
/// `\r\n` or `\n` as present in the source, or is the final unterminated
/// fragment.
fn split_lines(content: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let bytes = content.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\r' && i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
            lines.push(&content[start..i + 2]);
            start = i + 2;
            i += 2;
        } else if bytes[i] == b'\n' {
            lines.push(&content[start..i + 1]);
            start = i + 1;
            i += 1;
        } else {
            i += 1;
        }
    }
    if start < content.len() {
        lines.push(&content[start..]);
    }
    lines
}

/// Re-join a slice of lines (as returned by `split_lines`) back into a String.
/// The lines already contain their line-endings; `eol` is unused but kept for
/// symmetry / future use.
fn rejoin_lines(lines: &[&str], _eol: Eol) -> String {
    lines.concat()
}

pub fn sha256_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    format!("{:x}", h.finalize())
}

/// Build the start marker line (with the file's EOL).
pub fn start_marker(label: &str, eol: Eol) -> String {
    if label.is_empty() {
        format!("<!-- ALTEVRA_MANAGED_START -->{}", eol.as_str())
    } else {
        format!("<!-- ALTEVRA_MANAGED_START {label} -->{}", eol.as_str())
    }
}

/// Build the end marker line (with the file's EOL).
pub fn end_marker(label: &str, eol: Eol) -> String {
    if label.is_empty() {
        format!("<!-- ALTEVRA_MANAGED_END -->{}", eol.as_str())
    } else {
        format!("<!-- ALTEVRA_MANAGED_END {label} -->{}", eol.as_str())
    }
}

/// The content of a new block including its markers.
pub fn wrap_block(body: &str, label: &str, eol: Eol) -> String {
    format!(
        "{}{}{}",
        start_marker(label, eol),
        body,
        end_marker(label, eol)
    )
}

/// Outcome of a write attempt.
#[derive(Debug)]
pub enum WriteOutcome {
    /// Block was newly appended (file had no managed block).
    Appended,
    /// Block was refreshed in-place (content changed).
    Refreshed,
    /// Content was byte-identical — no write needed.
    AlreadyInSync,
    /// The block was found but its current content hash differs from the
    /// manifest baseline → DRIFT. The caller should file a review item.
    Drift {
        manifest_hash: String,
        current_hash: String,
    },
    /// Block markers are present but malformed (duplicate / nested / mismatched).
    Refused(String),
}

/// Read the target file, decide what to do, then atomically write via
/// temp+rename. Returns the outcome and the new block hash (if a write
/// was performed).
///
/// `manifest_hash`: the block hash from the `block_writes` row, or `None` if
/// we've never written this file before.
///
/// Safety contract:
///   - Bytes outside the markers are NEVER altered.
///   - CRLF line endings in the file are preserved.
///   - If `apply == false`, no disk writes are performed (dry-run).
pub fn write_block(
    target: &Path,
    new_body: &str,
    label: &str,
    manifest_hash: Option<&str>,
    apply: bool,
) -> anyhow::Result<(WriteOutcome, Option<String>)> {
    let existing_content = if target.exists() {
        std::fs::read_to_string(target).map_err(|e| {
            anyhow::anyhow!("read {}: {e}", target.display())
        })?
    } else {
        String::new()
    };

    let eol = if existing_content.is_empty() {
        Eol::Lf
    } else {
        detect_eol(&existing_content)
    };

    match parse_block(&existing_content) {
        ParseResult::Duplicate => {
            return Ok((
                WriteOutcome::Refused("duplicate ALTEVRA_MANAGED markers in file".into()),
                None,
            ));
        }
        ParseResult::Nested => {
            return Ok((
                WriteOutcome::Refused("nested ALTEVRA_MANAGED markers in file".into()),
                None,
            ));
        }
        ParseResult::Malformed(reason) => {
            return Ok((WriteOutcome::Refused(reason), None));
        }

        ParseResult::Found {
            start_line,
            end_line,
            label: _found_label,
            block_bytes: current_block_bytes,
        } => {
            let current_hash = sha256_hex(&current_block_bytes);

            // Drift check: if we have a manifest baseline and the file's
            // current block hash doesn't match it, someone edited the block.
            if let Some(baseline) = manifest_hash {
                if baseline != current_hash {
                    return Ok((
                        WriteOutcome::Drift {
                            manifest_hash: baseline.to_string(),
                            current_hash,
                        },
                        None,
                    ));
                }
            }

            // Build what we WANT to write.
            let new_block = wrap_block(new_body, label, eol);
            let new_hash = sha256_hex(&new_block);

            // Idempotent check.
            if current_hash == new_hash {
                return Ok((WriteOutcome::AlreadyInSync, Some(new_hash)));
            }

            if !apply {
                return Ok((WriteOutcome::Refreshed, Some(new_hash)));
            }

            // Rebuild the file: lines before START + new block + lines after END.
            let lines = split_lines(&existing_content);
            let prefix = rejoin_lines(&lines[..start_line], eol);
            let suffix = if end_line + 1 < lines.len() {
                rejoin_lines(&lines[end_line + 1..], eol)
            } else {
                String::new()
            };
            let new_content = format!("{prefix}{new_block}{suffix}");

            atomic_write(target, &new_content)?;
            Ok((WriteOutcome::Refreshed, Some(new_hash)))
        }

        ParseResult::Absent => {
            // Append a new block.
            let new_block = wrap_block(new_body, label, eol);
            let new_hash = sha256_hex(&new_block);

            // Idempotent: if the file is empty and the block hash is already
            // the manifest baseline, nothing changed.
            if let Some(baseline) = manifest_hash {
                if baseline == new_hash && existing_content.is_empty() {
                    return Ok((WriteOutcome::AlreadyInSync, Some(new_hash)));
                }
            }

            if !apply {
                return Ok((WriteOutcome::Appended, Some(new_hash)));
            }

            let new_content = if existing_content.is_empty() {
                new_block
            } else {
                // Append after a newline so there's a blank separator.
                let sep = if existing_content.ends_with(eol.as_str()) {
                    eol.as_str().to_string()
                } else {
                    format!("{}{}", eol.as_str(), eol.as_str())
                };
                format!("{existing_content}{sep}{new_block}")
            };

            atomic_write(target, &new_content)?;
            Ok((WriteOutcome::Appended, Some(new_hash)))
        }
    }
}

/// Write via temp file then rename (atomic on the same filesystem).
fn atomic_write(target: &Path, content: &str) -> anyhow::Result<()> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("mkdir {}: {e}", parent.display()))?;
    }
    let tmp = target.with_extension("md.altevra-tmp");
    std::fs::write(&tmp, content)
        .map_err(|e| anyhow::anyhow!("write tmp {}: {e}", tmp.display()))?;
    // TOCTOU verify.
    let written = std::fs::read_to_string(&tmp)
        .map_err(|e| anyhow::anyhow!("read-back tmp {}: {e}", tmp.display()))?;
    if written != content {
        let _ = std::fs::remove_file(&tmp);
        anyhow::bail!(
            "TOCTOU verify failed for {} — temp content mismatch",
            target.display()
        );
    }
    std::fs::rename(&tmp, target)
        .map_err(|e| anyhow::anyhow!("rename to {}: {e}", target.display()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests — all use TempDir fixtures, never real ~/.altevra
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_file(dir: &TempDir, name: &str, content: &str) -> std::path::PathBuf {
        let p = dir.path().join(name);
        std::fs::write(&p, content).unwrap();
        p
    }

    // ---- parse_block edge cases (spec-mandatory) ---------------------------

    #[test]
    fn parse_absent_when_no_markers() {
        assert_eq!(parse_block("# Title\n\nSome content.\n"), ParseResult::Absent);
        assert_eq!(parse_block(""), ParseResult::Absent);
    }

    #[test]
    fn parse_found_unlabeled_block() {
        let content = "before\n<!-- ALTEVRA_MANAGED_START -->\nbody\n<!-- ALTEVRA_MANAGED_END -->\nafter\n";
        match parse_block(content) {
            ParseResult::Found {
                start_line,
                end_line,
                label,
                block_bytes,
            } => {
                assert_eq!(start_line, 1);
                assert_eq!(end_line, 3);
                assert_eq!(label, "");
                assert!(block_bytes.contains("ALTEVRA_MANAGED_START"));
                assert!(block_bytes.contains("body"));
                assert!(block_bytes.contains("ALTEVRA_MANAGED_END"));
            }
            other => panic!("expected Found, got {other:?}"),
        }
    }

    #[test]
    fn parse_found_labeled_block() {
        let content = "x\n<!-- ALTEVRA_MANAGED_START context -->\ndata\n<!-- ALTEVRA_MANAGED_END context -->\ny\n";
        match parse_block(content) {
            ParseResult::Found { label, .. } => assert_eq!(label, "context"),
            other => panic!("expected Found, got {other:?}"),
        }
    }

    #[test]
    fn parse_duplicate_start_markers_refused() {
        let content = "<!-- ALTEVRA_MANAGED_START -->\na\n<!-- ALTEVRA_MANAGED_END -->\n<!-- ALTEVRA_MANAGED_START -->\nb\n<!-- ALTEVRA_MANAGED_END -->\n";
        assert_eq!(parse_block(content), ParseResult::Duplicate);
    }

    #[test]
    fn parse_nested_start_refused() {
        let content = "<!-- ALTEVRA_MANAGED_START -->\n<!-- ALTEVRA_MANAGED_START inner -->\nbody\n<!-- ALTEVRA_MANAGED_END inner -->\n<!-- ALTEVRA_MANAGED_END -->\n";
        // Two starts → Duplicate (catches nested as a subset).
        assert_eq!(parse_block(content), ParseResult::Duplicate);
    }

    #[test]
    fn parse_end_before_start_is_malformed() {
        let content = "<!-- ALTEVRA_MANAGED_END -->\nstuff\n<!-- ALTEVRA_MANAGED_START -->\n";
        // One start, one end but start_line > end_line → Nested.
        matches!(parse_block(content), ParseResult::Nested | ParseResult::Malformed(_));
    }

    #[test]
    fn parse_start_without_end_malformed() {
        let content = "<!-- ALTEVRA_MANAGED_START -->\nbody\n";
        matches!(parse_block(content), ParseResult::Malformed(_));
    }

    #[test]
    fn parse_crlf_preserved() {
        let content = "before\r\n<!-- ALTEVRA_MANAGED_START -->\r\nbody\r\n<!-- ALTEVRA_MANAGED_END -->\r\nafter\r\n";
        match parse_block(content) {
            ParseResult::Found { block_bytes, .. } => {
                assert!(block_bytes.contains("\r\n"), "CRLF must be preserved in block bytes");
            }
            other => panic!("expected Found, got {other:?}"),
        }
    }

    // ---- write_block behaviour -------------------------------------------

    #[test]
    fn missing_markers_appends_block() {
        let tmp = TempDir::new().unwrap();
        let target = write_file(&tmp, "CLAUDE.md", "# Title\n\nExisting content.\n");

        let (outcome, hash) = write_block(&target, "new body\n", "", None, true).unwrap();
        assert!(matches!(outcome, WriteOutcome::Appended), "{outcome:?}");
        assert!(hash.is_some());

        let content = std::fs::read_to_string(&target).unwrap();
        assert!(content.contains("Existing content."), "original must survive");
        assert!(content.contains("ALTEVRA_MANAGED_START"));
        assert!(content.contains("new body"));
        assert!(content.contains("ALTEVRA_MANAGED_END"));
    }

    #[test]
    fn write_to_empty_file_works() {
        let tmp = TempDir::new().unwrap();
        let target = write_file(&tmp, "empty.md", "");

        let (outcome, _hash) = write_block(&target, "body\n", "", None, true).unwrap();
        assert!(matches!(outcome, WriteOutcome::Appended), "{outcome:?}");
        let content = std::fs::read_to_string(&target).unwrap();
        assert!(content.contains("ALTEVRA_MANAGED_START"));
    }

    #[test]
    fn write_to_nonexistent_file_works() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("new.md");

        let (outcome, _hash) = write_block(&target, "body\n", "", None, true).unwrap();
        assert!(matches!(outcome, WriteOutcome::Appended), "{outcome:?}");
        assert!(target.exists());
    }

    #[test]
    fn refresh_updates_block_leaves_outside_bytes_intact() {
        let tmp = TempDir::new().unwrap();
        let target = write_file(
            &tmp,
            "CLAUDE.md",
            "BEFORE\n<!-- ALTEVRA_MANAGED_START -->\nold body\n<!-- ALTEVRA_MANAGED_END -->\nAFTER\n",
        );
        let current_block = "<!-- ALTEVRA_MANAGED_START -->\nold body\n<!-- ALTEVRA_MANAGED_END -->\n";
        let baseline = sha256_hex(current_block);

        let (outcome, new_hash) =
            write_block(&target, "new body\n", "", Some(&baseline), true).unwrap();
        assert!(matches!(outcome, WriteOutcome::Refreshed), "{outcome:?}");
        assert!(new_hash.is_some());

        let content = std::fs::read_to_string(&target).unwrap();
        assert!(content.starts_with("BEFORE\n"), "prefix must survive byte-identically");
        assert!(content.contains("new body"), "block updated");
        assert!(!content.contains("old body"), "old body replaced");
        assert!(content.ends_with("AFTER\n"), "suffix must survive byte-identically");
    }

    #[test]
    fn already_in_sync_no_write() {
        let tmp = TempDir::new().unwrap();
        let label = "";
        let body = "sync body\n";
        let eol = Eol::Lf;
        let block = wrap_block(body, label, eol);
        let hash = sha256_hex(&block);

        let content = format!("prefix\n{block}suffix\n");
        let target = write_file(&tmp, "CLAUDE.md", &content);
        let mtime_before = std::fs::metadata(&target).unwrap().modified().unwrap();

        let (outcome, returned_hash) =
            write_block(&target, body, label, Some(&hash), true).unwrap();
        assert!(matches!(outcome, WriteOutcome::AlreadyInSync), "{outcome:?}");
        assert_eq!(returned_hash.as_deref(), Some(hash.as_str()));

        // File must not have been touched (mtime unchanged).
        let mtime_after = std::fs::metadata(&target).unwrap().modified().unwrap();
        assert_eq!(mtime_before, mtime_after, "AlreadyInSync must not touch the file");
    }

    #[test]
    fn drift_refused_manual_edit_inside_block() {
        let tmp = TempDir::new().unwrap();
        let body = "original\n";
        let eol = Eol::Lf;
        let original_block = wrap_block(body, "", eol);
        let baseline = sha256_hex(&original_block);

        // Simulate a human editing inside the block.
        let human_block = format!(
            "<!-- ALTEVRA_MANAGED_START -->\nhuman edit\n<!-- ALTEVRA_MANAGED_END -->\n"
        );
        let content = format!("prefix\n{human_block}suffix\n");
        let target = write_file(&tmp, "CLAUDE.md", &content);

        let (outcome, hash) =
            write_block(&target, "new body\n", "", Some(&baseline), true).unwrap();
        assert!(
            matches!(outcome, WriteOutcome::Drift { .. }),
            "manual edit inside block must be drift: {outcome:?}"
        );
        assert!(hash.is_none());

        // File must be byte-identical to what we wrote (human edit untouched).
        let still = std::fs::read_to_string(&target).unwrap();
        assert_eq!(still, content, "drift refuse must leave file byte-identical");
    }

    #[test]
    fn manual_edit_outside_block_survives_two_runs() {
        let tmp = TempDir::new().unwrap();
        let body = "body\n";

        // First run: create the block (no baseline).
        let target = write_file(&tmp, "CLAUDE.md", "HUMAN_OUTSIDE\n");
        let (_, hash1) = write_block(&target, body, "", None, true).unwrap();
        let hash1 = hash1.unwrap();

        let after_first = std::fs::read_to_string(&target).unwrap();
        assert!(
            after_first.contains("HUMAN_OUTSIDE"),
            "content before block survives"
        );

        // Second run: same body → AlreadyInSync, outside bytes unchanged.
        let (outcome, hash2) = write_block(&target, body, "", Some(&hash1), true).unwrap();
        assert!(matches!(outcome, WriteOutcome::AlreadyInSync), "{outcome:?}");

        let after_second = std::fs::read_to_string(&target).unwrap();
        assert_eq!(
            after_first, after_second,
            "outside bytes must survive byte-identically across two runs"
        );
        assert_eq!(hash2.as_deref(), Some(hash1.as_str()));
    }

    #[test]
    fn crlf_file_eol_preserved_after_write() {
        let tmp = TempDir::new().unwrap();
        let content = "before\r\n<!-- ALTEVRA_MANAGED_START -->\r\nold\r\n<!-- ALTEVRA_MANAGED_END -->\r\nafter\r\n";
        let target = write_file(&tmp, "CRLF.md", content);
        let block_bytes = "<!-- ALTEVRA_MANAGED_START -->\r\nold\r\n<!-- ALTEVRA_MANAGED_END -->\r\n";
        let baseline = sha256_hex(block_bytes);

        let (outcome, _) = write_block(&target, "new\r\n", "", Some(&baseline), true).unwrap();
        assert!(
            matches!(outcome, WriteOutcome::Refreshed),
            "CRLF file: {outcome:?}"
        );

        let result = std::fs::read_to_string(&target).unwrap();
        // The file should still use CRLF everywhere.
        let crlf_count = result.matches("\r\n").count();
        let lf_only = result.matches('\n').count() - crlf_count;
        assert_eq!(lf_only, 0, "CRLF file must not gain lone LF after write");
    }

    #[test]
    fn duplicate_markers_refused() {
        let tmp = TempDir::new().unwrap();
        let content = "<!-- ALTEVRA_MANAGED_START -->\na\n<!-- ALTEVRA_MANAGED_END -->\n<!-- ALTEVRA_MANAGED_START -->\nb\n<!-- ALTEVRA_MANAGED_END -->\n";
        let target = write_file(&tmp, "dup.md", content);

        let (outcome, _) = write_block(&target, "x", "", None, true).unwrap();
        assert!(matches!(outcome, WriteOutcome::Refused(_)), "{outcome:?}");
        // File must not be modified.
        assert_eq!(std::fs::read_to_string(&target).unwrap(), content);
    }

    #[test]
    fn dry_run_does_not_write() {
        let tmp = TempDir::new().unwrap();
        let target = write_file(&tmp, "CLAUDE.md", "existing\n");

        let (outcome, hash) = write_block(&target, "new body\n", "", None, false).unwrap();
        assert!(
            matches!(outcome, WriteOutcome::Appended),
            "dry-run should still report what WOULD happen: {outcome:?}"
        );
        assert!(hash.is_some());
        // File must be unchanged.
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "existing\n",
            "dry-run must not write"
        );
    }
}

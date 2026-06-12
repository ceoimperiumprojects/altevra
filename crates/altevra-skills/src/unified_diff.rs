//! Minimal, strict unified-diff applier (E2 prompt self-tweaking).
//!
//! A `prompt_tweak` proposal carries a UNIFIED-DIFF body that, when approved,
//! is applied to the MANAGED REGION of a resident-mode prompt file via the R5
//! [`crate::block_writer`]. This module turns that diff into the new managed
//! body text — it NEVER touches disk; the block writer owns the file.
//!
//! ## Why a bespoke parser (and not a diff crate)
//!
//! The contract is narrow and security-shaped: a tweak edits ONLY the managed
//! body (the text between Altevra's markers), so the diff applies to a small,
//! known string. We want STRICT validation — a context line that does not match
//! the source, an out-of-range hunk, or a malformed header → REFUSE (the
//! approve path then leaves the file byte-identical). A permissive fuzzy-match
//! applier is exactly the wrong tool for a self-modifying-prompt gate.
//!
//! ## Supported grammar (a practical subset of unified diff)
//!
//! ```text
//! --- a/<path>            (optional file header, ignored)
//! +++ b/<path>            (optional file header, ignored)
//! @@ -l,s +l,s @@         (hunk header; the line numbers ARE validated)
//!  context line           (leading space)
//! -removed line           (leading '-')
//! +added line             (leading '+')
//! ```
//!
//! `\ No newline at end of file` markers are accepted and ignored (the managed
//! body is always normalized to end in a newline by the block writer).

/// One parsed hunk: 1-based old/new start lines + the body lines (with their
/// leading op char preserved).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Hunk {
    old_start: usize,
    old_count: usize,
    new_start: usize,
    new_count: usize,
    /// Body lines INCLUDING the leading op character (' ', '-', '+').
    lines: Vec<String>,
}

/// Why a diff was refused. Carries a human-readable reason for the review item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffError {
    /// No `@@` hunk header was found — the body is not a unified diff.
    NoHunks,
    /// A hunk header could not be parsed.
    BadHeader(String),
    /// A body line did not start with ' ', '-', '+', or '\\'.
    BadLine(String),
    /// A context/removed line did not match the source at the expected position.
    ContextMismatch { expected: String, found: String },
    /// A hunk referenced a line past the end of the source.
    OutOfRange(String),
    /// The applied hunk's line count disagreed with its header.
    CountMismatch(String),
}

impl std::fmt::Display for DiffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiffError::NoHunks => write!(f, "diff has no @@ hunks"),
            DiffError::BadHeader(h) => write!(f, "malformed hunk header: {h}"),
            DiffError::BadLine(l) => write!(f, "malformed diff line: {l}"),
            DiffError::ContextMismatch { expected, found } => write!(
                f,
                "context mismatch — expected {expected:?}, source had {found:?}"
            ),
            DiffError::OutOfRange(m) => write!(f, "hunk out of range: {m}"),
            DiffError::CountMismatch(m) => write!(f, "hunk line count mismatch: {m}"),
        }
    }
}

impl std::error::Error for DiffError {}

/// Parse a `@@ -l,s +l,s @@` header. `s` is optional (defaults to 1).
fn parse_hunk_header(line: &str) -> Result<(usize, usize, usize, usize), DiffError> {
    // Grammar: @@ -old_start[,old_count] +new_start[,new_count] @@ [section]
    let inner = line
        .strip_prefix("@@")
        .and_then(|s| s.trim_start().strip_prefix('-'))
        .ok_or_else(|| DiffError::BadHeader(line.to_string()))?;
    // Split on the "+": "old_part +new_part @@ ..."
    let plus = inner
        .find(" +")
        .ok_or_else(|| DiffError::BadHeader(line.to_string()))?;
    let old_part = &inner[..plus];
    let after_plus = &inner[plus + 2..];
    // new_part runs up to the closing "@@".
    let new_part = after_plus
        .split("@@")
        .next()
        .unwrap_or("")
        .trim();

    let (old_start, old_count) = parse_range(old_part.trim())
        .ok_or_else(|| DiffError::BadHeader(line.to_string()))?;
    let (new_start, new_count) =
        parse_range(new_part).ok_or_else(|| DiffError::BadHeader(line.to_string()))?;
    Ok((old_start, old_count, new_start, new_count))
}

/// Parse `l` or `l,s` into `(start, count)`.
fn parse_range(s: &str) -> Option<(usize, usize)> {
    let mut parts = s.split(',');
    let start: usize = parts.next()?.trim().parse().ok()?;
    let count: usize = match parts.next() {
        Some(c) => c.trim().parse().ok()?,
        None => 1,
    };
    Some((start, count))
}

/// Parse the full diff body into hunks. Lines before the first `@@` (the
/// optional `---`/`+++` file headers, or any preamble) are ignored.
fn parse_hunks(diff: &str) -> Result<Vec<Hunk>, DiffError> {
    let mut hunks: Vec<Hunk> = Vec::new();
    let mut current: Option<Hunk> = None;

    for raw in diff.lines() {
        if raw.starts_with("@@") {
            if let Some(h) = current.take() {
                hunks.push(h);
            }
            let (os, oc, ns, nc) = parse_hunk_header(raw)?;
            current = Some(Hunk {
                old_start: os,
                old_count: oc,
                new_start: ns,
                new_count: nc,
                lines: Vec::new(),
            });
            continue;
        }
        // Skip file headers + index lines that appear BEFORE the first hunk.
        if current.is_none() {
            continue;
        }
        // Inside a hunk: classify the line.
        if raw.starts_with("\\") {
            // "\ No newline at end of file" — accepted, ignored.
            continue;
        }
        if raw.is_empty() {
            // A truly empty line in the diff = a context line that is the empty
            // string (some tools emit this). Treat it as a blank context line.
            current.as_mut().unwrap().lines.push(" ".to_string());
            continue;
        }
        match raw.as_bytes()[0] {
            b' ' | b'-' | b'+' => current.as_mut().unwrap().lines.push(raw.to_string()),
            _ => return Err(DiffError::BadLine(raw.to_string())),
        }
    }
    if let Some(h) = current.take() {
        hunks.push(h);
    }
    if hunks.is_empty() {
        return Err(DiffError::NoHunks);
    }
    Ok(hunks)
}

/// Apply a unified diff to `source`, returning the patched text. STRICT:
/// any context/removed-line mismatch, out-of-range hunk, malformed header, or
/// header/body count disagreement → [`DiffError`] (the caller refuses the
/// write, leaving the target byte-identical).
///
/// Newline handling: the source is split on `\n`; the result is re-joined with
/// `\n` and given a trailing `\n` (the managed body is always newline-terminated
/// by the block writer). This is intentional — the managed region is Altevra's,
/// not human-owned, so canonicalizing its EOL is safe.
pub fn apply_unified_diff(source: &str, diff: &str) -> Result<String, DiffError> {
    let hunks = parse_hunks(diff)?;

    // Work on a line view of the source WITHOUT trailing-newline artifacts.
    let src_lines: Vec<&str> = if source.is_empty() {
        Vec::new()
    } else {
        source.strip_suffix('\n').unwrap_or(source).split('\n').collect()
    };

    let mut out: Vec<String> = Vec::new();
    // 0-based cursor into src_lines; tracks where we've copied up to.
    let mut cursor: usize = 0;

    for h in &hunks {
        // old_start is 1-based; a count of 0 means a pure insertion AT old_start.
        let hunk_old_idx = h.old_start.saturating_sub(1);
        if hunk_old_idx > src_lines.len() {
            return Err(DiffError::OutOfRange(format!(
                "old_start {} > source length {}",
                h.old_start,
                src_lines.len()
            )));
        }
        // Copy untouched lines between the cursor and this hunk's start.
        if hunk_old_idx < cursor {
            return Err(DiffError::OutOfRange(format!(
                "hunk at {} overlaps already-consumed line {}",
                h.old_start, cursor
            )));
        }
        for line in &src_lines[cursor..hunk_old_idx] {
            out.push((*line).to_string());
        }
        cursor = hunk_old_idx;

        let mut consumed_old = 0usize;
        let mut produced_new = 0usize;
        for body in &h.lines {
            let (op, text) = body.split_at(1);
            match op {
                " " => {
                    // Context: must match source, copy through.
                    let found = src_lines.get(cursor).copied().ok_or_else(|| {
                        DiffError::OutOfRange(format!(
                            "context past end of source at line {}",
                            cursor + 1
                        ))
                    })?;
                    if found != text {
                        return Err(DiffError::ContextMismatch {
                            expected: text.to_string(),
                            found: found.to_string(),
                        });
                    }
                    out.push(text.to_string());
                    cursor += 1;
                    consumed_old += 1;
                    produced_new += 1;
                }
                "-" => {
                    // Removed: must match source, skip.
                    let found = src_lines.get(cursor).copied().ok_or_else(|| {
                        DiffError::OutOfRange(format!(
                            "removal past end of source at line {}",
                            cursor + 1
                        ))
                    })?;
                    if found != text {
                        return Err(DiffError::ContextMismatch {
                            expected: text.to_string(),
                            found: found.to_string(),
                        });
                    }
                    cursor += 1;
                    consumed_old += 1;
                }
                "+" => {
                    // Added: emit, consume nothing from source.
                    out.push(text.to_string());
                    produced_new += 1;
                }
                _ => return Err(DiffError::BadLine(body.clone())),
            }
        }

        // Validate against the header counts (a self-modify gate is paranoid).
        if consumed_old != h.old_count {
            return Err(DiffError::CountMismatch(format!(
                "hunk header old_count={} but body consumed {}",
                h.old_count, consumed_old
            )));
        }
        if produced_new != h.new_count {
            return Err(DiffError::CountMismatch(format!(
                "hunk header new_count={} but body produced {}",
                h.new_count, produced_new
            )));
        }
    }

    // Copy any trailing untouched lines.
    for line in &src_lines[cursor..] {
        out.push((*line).to_string());
    }

    let mut joined = out.join("\n");
    if !joined.is_empty() {
        joined.push('\n');
    }
    Ok(joined)
}

/// Validate a diff is well-formed and APPLIES CLEANLY to `source` without
/// committing — used by the proposal path to reject malformed diffs at propose
/// time (so a bad diff never reaches the review queue). Returns `Ok(())` if the
/// diff parses and applies.
pub fn validate_unified_diff(source: &str, diff: &str) -> Result<(), DiffError> {
    apply_unified_diff(source, diff).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_simple_replacement() {
        let src = "line one\nline two\nline three\n";
        let diff = "@@ -2,1 +2,1 @@\n-line two\n+LINE TWO EDITED\n";
        let out = apply_unified_diff(src, diff).unwrap();
        assert_eq!(out, "line one\nLINE TWO EDITED\nline three\n");
    }

    #[test]
    fn applies_with_context_lines() {
        let src = "a\nb\nc\nd\n";
        let diff = "@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n";
        let out = apply_unified_diff(src, diff).unwrap();
        assert_eq!(out, "a\nB\nc\nd\n");
    }

    #[test]
    fn applies_pure_insertion() {
        let src = "first\nsecond\n";
        // insert after line 1: header new_count = 2 (context + added).
        let diff = "@@ -1,1 +1,2 @@\n first\n+inserted\n";
        let out = apply_unified_diff(src, diff).unwrap();
        assert_eq!(out, "first\ninserted\nsecond\n");
    }

    #[test]
    fn applies_pure_deletion() {
        let src = "keep1\ndrop\nkeep2\n";
        let diff = "@@ -1,3 +1,2 @@\n keep1\n-drop\n keep2\n";
        let out = apply_unified_diff(src, diff).unwrap();
        assert_eq!(out, "keep1\nkeep2\n");
    }

    #[test]
    fn ignores_file_headers() {
        let src = "x\ny\n";
        let diff = "--- a/foo.md\n+++ b/foo.md\n@@ -1,1 +1,1 @@\n-x\n+X\n";
        let out = apply_unified_diff(src, diff).unwrap();
        assert_eq!(out, "X\ny\n");
    }

    #[test]
    fn refuses_context_mismatch() {
        let src = "alpha\nbeta\n";
        // Claims to remove "WRONG" which is not in the source.
        let diff = "@@ -1,1 +1,1 @@\n-WRONG\n+new\n";
        let err = apply_unified_diff(src, diff).unwrap_err();
        assert!(matches!(err, DiffError::ContextMismatch { .. }), "{err:?}");
    }

    #[test]
    fn refuses_no_hunks() {
        let err = apply_unified_diff("a\n", "not a diff at all\n").unwrap_err();
        assert_eq!(err, DiffError::NoHunks);
    }

    #[test]
    fn refuses_bad_header() {
        let err = apply_unified_diff("a\n", "@@ garbage @@\n-a\n+b\n").unwrap_err();
        assert!(matches!(err, DiffError::BadHeader(_)), "{err:?}");
    }

    #[test]
    fn refuses_out_of_range() {
        let src = "only one line\n";
        let diff = "@@ -50,1 +50,1 @@\n-x\n+y\n";
        let err = apply_unified_diff(src, diff).unwrap_err();
        assert!(matches!(err, DiffError::OutOfRange(_)), "{err:?}");
    }

    #[test]
    fn refuses_count_mismatch() {
        let src = "a\nb\n";
        // header says old_count=2 but body only consumes 1.
        let diff = "@@ -1,2 +1,1 @@\n-a\n+A\n";
        let err = apply_unified_diff(src, diff).unwrap_err();
        assert!(matches!(err, DiffError::CountMismatch(_)), "{err:?}");
    }

    #[test]
    fn validate_only_does_not_mutate() {
        let src = "a\nb\n";
        let diff = "@@ -1,1 +1,1 @@\n-a\n+A\n";
        assert!(validate_unified_diff(src, diff).is_ok());
        // validating a malformed diff returns Err but does not panic.
        assert!(validate_unified_diff(src, "garbage").is_err());
    }

    #[test]
    fn tolerates_no_newline_marker() {
        let src = "a\nb";
        let diff = "@@ -2,1 +2,1 @@\n-b\n\\ No newline at end of file\n+B\n";
        let out = apply_unified_diff(src, diff).unwrap();
        assert_eq!(out, "a\nB\n");
    }
}

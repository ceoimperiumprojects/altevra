//! Imperium mirror WRITER (P0 task E2, §2.14 D4, R10 Q-VAULT).
//!
//! The PURE policy + renderer lives in [`altevra_core::mirror::render_mirror`]: it
//! decides whether an object may be mirrored and, if so, produces the managed
//! markdown + relative vault path. **This module is the writer side** that takes
//! the rendered doc and SAFELY persists it under a vault root — with three hard
//! guarantees:
//!
//! 1. **DRY-RUN by default.** [`write_mirror`] only writes when explicitly told
//!    to (`dry_run = false`). Every CLI seam built on top of it (e.g. the
//!    `altevra mirror plan` verb) MUST default `dry_run = true`. The plan
//!    surfaces target path, content checksum, and bytes — nothing hits disk.
//!
//! 2. **Never overwrites human edits.** Before writing, the writer re-reads the
//!    existing target file and refuses unless it sees (a) the Altevra-managed
//!    marker AND (b) a stamped sha256 that still matches the on-disk content.
//!    Either condition broken ⇒ a human edited the file — refuse, never clobber.
//!
//! 3. **D4 defense in depth.** Even if a caller wraps the renderer and forgets
//!    the high-water/Confidential+ gate, this writer re-enforces D4 itself by
//!    calling `render_mirror` here and treating its `None` as a hard `Skipped`.
//!    The Obsidian vault never receives high-water material via this path.
//!
//! The writer is intentionally tiny + keyless. The live CLI seam is presence-
//! gated upstream (`require_human_presence`) before any non-dry-run call is
//! ever wired; this module's API surface is the gate-friendly primitive.

use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use thiserror::Error;

use altevra_core::envelope::Envelope;
use altevra_core::mirror::{render_mirror, MirrorDoc};

/// Marker we stamp on every Altevra-written mirror file (mirrors what
/// `render_mirror` already emits — we re-check it on disk to detect human
/// authorship). If a target file does NOT carry this marker, we treat it as
/// human-authored and refuse to overwrite.
const MANAGED_MARKER: &str = "<!-- ALTEVRA_MANAGED: true -->";

/// Extra stamp line emitted by the writer (NOT by `render_mirror`) carrying the
/// sha256 of the rendered body. On a future overwrite we recompute the sha of
/// the on-disk content with the stamp line stripped — any mismatch means a
/// human (or another tool) edited the file in place. Refuse.
const SHA_STAMP_PREFIX: &str = "<!-- ALTEVRA_MIRROR_SHA256: ";
const SHA_STAMP_SUFFIX: &str = " -->";

/// Errors that can fault the writer itself (I/O). Refusal-by-policy is NOT an
/// error — it's a [`WriteOutcome::Refused`] / [`WriteOutcome::Skipped`].
#[derive(Debug, Error)]
pub enum MirrorWriterError {
    #[error("failed to read existing target {path}: {source}")]
    ReadTarget {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to create parent dir {path}: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// What the writer would (or did) do for a single object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteOutcome {
    /// Dry-run plan: nothing written. `bytes` is the would-write byte count.
    Planned {
        target: PathBuf,
        relative_path: String,
        sha256: String,
        bytes: usize,
    },
    /// The file was actually written to disk (`dry_run = false`).
    Wrote {
        target: PathBuf,
        relative_path: String,
        sha256: String,
        bytes: usize,
    },
    /// Object is not mirrorable by policy (D4: high-water / Confidential+, or
    /// renderer otherwise returned `None`). No file touched, no plan emitted.
    Skipped { reason: SkipReason },
    /// Mirrorable, but the existing on-disk target shows signs of human edits
    /// (no managed marker, or stamped checksum drifted). No file touched.
    Refused {
        target: PathBuf,
        reason: RefuseReason,
    },
}

/// Why a mirror was skipped (mirrorability gate — D4 second line of defense).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    /// Renderer refused: confidential+ or high-water domain. NEVER mirrors.
    HighWaterNeverMirrors,
}

impl SkipReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            SkipReason::HighWaterNeverMirrors => "high_water_never_mirrors",
        }
    }
}

/// Why a mirror write was refused (human-edit protection).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefuseReason {
    /// The existing target file does not carry the Altevra-managed marker —
    /// it's human-authored. Never clobber.
    NotAltevraManaged,
    /// The marker is present but the stamped sha256 no longer matches the
    /// on-disk content — a human edited it after Altevra last wrote it.
    HumanEditedSinceLastMirror,
}

impl RefuseReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            RefuseReason::NotAltevraManaged => "not_altevra_managed",
            RefuseReason::HumanEditedSinceLastMirror => "human_edited_since_last_mirror",
        }
    }
}

/// Compose the final file content = rendered content + sha256 stamp line.
///
/// The stamp is `<!-- ALTEVRA_MIRROR_SHA256: <hex> -->` and carries the sha256
/// of the rendered content (without the stamp). On a future overwrite, the
/// drift check strips the stamp line and recomputes — any change to the body
/// invalidates the match.
fn stamp_content(rendered: &str) -> (String, String) {
    let sha = sha256_hex(rendered.as_bytes());
    // Ensure body ends with exactly one newline before the stamp line.
    let mut out = String::with_capacity(rendered.len() + sha.len() + 64);
    out.push_str(rendered);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(SHA_STAMP_PREFIX);
    out.push_str(&sha);
    out.push_str(SHA_STAMP_SUFFIX);
    out.push('\n');
    (out, sha)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

/// Strip the last `ALTEVRA_MIRROR_SHA256` stamp line (if any) from `s` and
/// return (body_without_stamp, stamped_sha). The stamp must be the very last
/// non-empty line; anything else means the file was modified.
fn extract_stamp(s: &str) -> Option<(String, String)> {
    // Find a stamp line; we accept it as the final non-empty line of the file
    // (writer guarantees trailing newline after stamp).
    let mut lines: Vec<&str> = s.lines().collect();
    // Drop trailing empty lines.
    while lines.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
        lines.pop();
    }
    let last = lines.last().copied()?;
    let inner = last
        .strip_prefix(SHA_STAMP_PREFIX)
        .and_then(|r| r.strip_suffix(SHA_STAMP_SUFFIX))?
        .trim();
    if inner.is_empty() {
        return None;
    }
    lines.pop();
    // Re-glue (preserving any in-body newlines). The render adds a trailing `\n`
    // before the stamp, so dropping the stamp leaves us with that trailing
    // newline — we must restore it to match the original sha input.
    let mut body = lines.join("\n");
    if !body.ends_with('\n') {
        body.push('\n');
    }
    Some((body, inner.to_string()))
}

/// Plan-only computation (no I/O beyond reading the target to assess drift).
/// Surfaces target path, sha256, would-write bytes — never writes.
pub fn plan_mirror(
    root: &Path,
    env: &Envelope,
    title: &str,
    body: &str,
) -> Result<WriteOutcome, MirrorWriterError> {
    write_mirror(root, env, title, body, true)
}

/// Write (or plan) a mirror file for `(env, title, body)` under `root`.
///
/// `dry_run = true` (the live default) emits a [`WriteOutcome::Planned`] and
/// touches nothing. `dry_run = false` actually writes — but only after passing
/// BOTH the D4 mirrorability re-check AND the human-edit drift detector. Any
/// drift → [`WriteOutcome::Refused`], target file untouched.
pub fn write_mirror(
    root: &Path,
    env: &Envelope,
    title: &str,
    body: &str,
    dry_run: bool,
) -> Result<WriteOutcome, MirrorWriterError> {
    // D4 second line of defense: even if a caller bypassed the renderer
    // elsewhere, we re-ask the canonical policy here. `None` ⇒ never mirror.
    let MirrorDoc {
        relative_path,
        content,
    } = match render_mirror(env, title, body) {
        Some(d) => d,
        None => {
            return Ok(WriteOutcome::Skipped {
                reason: SkipReason::HighWaterNeverMirrors,
            });
        }
    };

    let target = root.join(&relative_path);
    let (stamped, sha) = stamp_content(&content);
    let bytes = stamped.len();

    // Human-edit detection: only relevant if the target already exists. A
    // missing target is a fresh write and trivially safe.
    if target.exists() {
        let existing =
            fs::read_to_string(&target).map_err(|e| MirrorWriterError::ReadTarget {
                path: target.clone(),
                source: e,
            })?;
        // 1) Marker absent ⇒ human-authored file, refuse.
        if !existing.contains(MANAGED_MARKER) {
            return Ok(WriteOutcome::Refused {
                target,
                reason: RefuseReason::NotAltevraManaged,
            });
        }
        // 2) Marker present — recompute the stamped sha and compare. Any drift
        //    (no stamp line OR stamped value doesn't match current content)
        //    means a human edited the file after Altevra last wrote it.
        let drifted = match extract_stamp(&existing) {
            None => true,
            Some((body_without_stamp, stamped_sha)) => {
                stamped_sha != sha256_hex(body_without_stamp.as_bytes())
            }
        };
        if drifted {
            return Ok(WriteOutcome::Refused {
                target,
                reason: RefuseReason::HumanEditedSinceLastMirror,
            });
        }
    }

    if dry_run {
        return Ok(WriteOutcome::Planned {
            target,
            relative_path,
            sha256: sha,
            bytes,
        });
    }

    // ---- Live write (presence-gated upstream). ----
    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            fs::create_dir_all(parent).map_err(|e| MirrorWriterError::CreateDir {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
    }
    fs::write(&target, &stamped).map_err(|e| MirrorWriterError::Write {
        path: target.clone(),
        source: e,
    })?;
    Ok(WriteOutcome::Wrote {
        target,
        relative_path,
        sha256: sha,
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use altevra_core::domain::Domain;
    use altevra_core::envelope::{Provenance, ProvenanceOrigin};
    use altevra_core::security::Sensitivity;
    use chrono::{DateTime, Utc};
    use sha2::{Digest, Sha256};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn env(id: &str, ty: &str, domain: Domain, sens: Sensitivity) -> Envelope {
        let mut e = Envelope::new(
            id,
            ty,
            now(),
            Provenance::new(ProvenanceOrigin::PavleDirect),
        );
        e.domain = domain;
        e.sensitivity = sens;
        e
    }

    fn sha_of(p: &PathBuf) -> Option<String> {
        let bytes = std::fs::read(p).ok()?;
        let mut h = Sha256::new();
        h.update(&bytes);
        Some(hex::encode(h.finalize()))
    }

    // ---- Real-vault canary: assert ~/Obsidian/Imperium is byte-untouched ----
    //
    // Records sha256 of a few canonical real files (if they exist) at the
    // start of the test and re-checks at the end. If the vault doesn't exist
    // on this host (e.g. CI), the canary degrades to a no-op pre-existence
    // check (every recorded file is still absent at the end). Either way: the
    // tests must NEVER produce a write under ~/Obsidian/Imperium.
    struct VaultCanary {
        files: Vec<(PathBuf, Option<String>)>,
        root_existed: bool,
        root: PathBuf,
    }

    impl VaultCanary {
        fn arm() -> Self {
            let root = std::env::var("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/"))
                .join("Obsidian")
                .join("Imperium");
            let root_existed = root.exists();
            let mut files: Vec<(PathBuf, Option<String>)> = Vec::new();
            for rel in [
                "Memory/Decisions.md",
                "Memory/Learnings.md",
                "Memory/People.md",
                "CLAUDE.md",
            ] {
                let p = root.join(rel);
                let sha = sha_of(&p);
                files.push((p, sha));
            }
            VaultCanary {
                files,
                root_existed,
                root,
            }
        }

        fn assert_untouched(&self) {
            // The vault root must still be exactly what it was: either
            // pre-existing (in which case file shas must match) or absent
            // (in which case the writer must not have created it).
            assert_eq!(
                self.root.exists(),
                self.root_existed,
                "vault root presence must not change during a mirror test (root={})",
                self.root.display()
            );
            for (p, before) in &self.files {
                let after = sha_of(p);
                assert_eq!(
                    &after, before,
                    "canary file '{}' was modified by a mirror test — REAL vault must be byte-untouched",
                    p.display()
                );
            }
        }
    }

    #[test]
    fn mirror_dry_run_writes_nothing() {
        let canary = VaultCanary::arm();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        let outcome = write_mirror(
            root,
            &env("d1", "decision", Domain::Business, Sensitivity::Internal),
            "Adopt SQLite",
            "Local-first store.",
            true, // dry_run
        )
        .unwrap();

        match outcome {
            WriteOutcome::Planned {
                target,
                relative_path,
                sha256,
                bytes,
            } => {
                assert_eq!(relative_path, "08-decisions/d1.md");
                assert_eq!(target, root.join("08-decisions/d1.md"));
                assert!(bytes > 0);
                assert_eq!(sha256.len(), 64, "sha256 hex");
                assert!(!target.exists(), "DRY-RUN must NOT write");
            }
            other => panic!("expected Planned, got {other:?}"),
        }

        // Belt-and-braces: tmp root has no file at all after dry-run.
        assert!(
            !root.join("08-decisions").exists(),
            "no directory should be created on dry-run"
        );

        canary.assert_untouched();
    }

    #[test]
    fn mirror_skips_high_water_and_human_edits() {
        let canary = VaultCanary::arm();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        // ---- D4 second line: high-water domain is Skipped (HighWaterNeverMirrors) ----
        for d in [
            Domain::Health,
            Domain::Relationship,
            Domain::Personal,
            Domain::Financial,
        ] {
            let outcome = write_mirror(
                root,
                &env("h", "learning", d.clone(), Sensitivity::Internal),
                "t",
                "b",
                false, // even with dry_run=false, must NOT write
            )
            .unwrap();
            assert_eq!(
                outcome,
                WriteOutcome::Skipped {
                    reason: SkipReason::HighWaterNeverMirrors,
                },
                "high-water domain {d:?} must be Skipped, not written"
            );
        }

        // ---- D4 second line: confidential+ is Skipped too ----
        let outcome = write_mirror(
            root,
            &env(
                "c1",
                "decision",
                Domain::Business,
                Sensitivity::Confidential,
            ),
            "Deal",
            "secret",
            false,
        )
        .unwrap();
        assert_eq!(
            outcome,
            WriteOutcome::Skipped {
                reason: SkipReason::HighWaterNeverMirrors,
            },
            "confidential decision must be Skipped"
        );

        // ---- Human-edit detection: target without the managed marker → Refused ----
        let human_path = root.join("08-decisions/d2.md");
        std::fs::create_dir_all(human_path.parent().unwrap()).unwrap();
        std::fs::write(&human_path, "# Pavle's own notes\nhand-written.\n").unwrap();

        let outcome = write_mirror(
            root,
            &env("d2", "decision", Domain::Business, Sensitivity::Internal),
            "Adopt SQLite",
            "Local-first store.",
            false,
        )
        .unwrap();
        match outcome {
            WriteOutcome::Refused { target, reason } => {
                assert_eq!(target, human_path);
                assert_eq!(reason, RefuseReason::NotAltevraManaged);
            }
            other => panic!("expected Refused(NotAltevraManaged), got {other:?}"),
        }
        // The human file is byte-identical to before.
        assert_eq!(
            std::fs::read_to_string(&human_path).unwrap(),
            "# Pavle's own notes\nhand-written.\n"
        );

        // Whole tree under root: only the human file exists (no other writes leaked).
        assert!(human_path.exists());
        let count = walk_count(root);
        assert_eq!(count, 1, "only the pre-existing human file is on disk");

        canary.assert_untouched();
    }

    #[test]
    fn mirror_refuses_drifted_target() {
        let canary = VaultCanary::arm();
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        let envb = env("d3", "decision", Domain::Business, Sensitivity::Internal);

        // 1) Live write produces a managed + stamped file.
        let out = write_mirror(root, &envb, "T", "body v1", false).unwrap();
        let target = match out {
            WriteOutcome::Wrote { target, .. } => target,
            other => panic!("expected Wrote, got {other:?}"),
        };
        assert!(target.exists());
        let on_disk = std::fs::read_to_string(&target).unwrap();
        assert!(on_disk.contains(MANAGED_MARKER));
        assert!(on_disk.contains(SHA_STAMP_PREFIX));

        // 2) Re-writing the SAME body is safe: the stamped sha still matches.
        let out2 = write_mirror(root, &envb, "T", "body v1", false).unwrap();
        match out2 {
            WriteOutcome::Wrote { .. } => {}
            other => panic!("expected idempotent Wrote, got {other:?}"),
        }

        // 3) Simulate a human edit: append a line into the managed file.
        let mut tampered = std::fs::read_to_string(&target).unwrap();
        tampered.push_str("Pavle's hand edit.\n");
        std::fs::write(&target, &tampered).unwrap();

        // 4) Next write — even with a fresh body — must REFUSE, never clobber.
        let out3 = write_mirror(root, &envb, "T", "body v2 (would clobber)", false).unwrap();
        match out3 {
            WriteOutcome::Refused { target: t, reason } => {
                assert_eq!(t, target);
                assert_eq!(reason, RefuseReason::HumanEditedSinceLastMirror);
            }
            other => panic!("expected Refused(HumanEditedSinceLastMirror), got {other:?}"),
        }
        // The tampered file is byte-identical — never touched by the refusal.
        assert_eq!(std::fs::read_to_string(&target).unwrap(), tampered);

        // 5) Same drift surfaces under dry-run too (the policy precedes write).
        let out4 = write_mirror(root, &envb, "T", "body v2", true).unwrap();
        match out4 {
            WriteOutcome::Refused { reason, .. } => {
                assert_eq!(reason, RefuseReason::HumanEditedSinceLastMirror);
            }
            other => panic!("expected Refused under dry-run too, got {other:?}"),
        }

        canary.assert_untouched();
    }

    #[test]
    fn plan_mirror_is_dry_run_alias() {
        let tmp = TempDir::new().unwrap();
        let out = plan_mirror(
            tmp.path(),
            &env("p1", "wiki_page", Domain::Public, Sensitivity::Public),
            "Wiki",
            "body",
        )
        .unwrap();
        match out {
            WriteOutcome::Planned { target, .. } => assert!(!target.exists()),
            other => panic!("plan_mirror must be Planned, got {other:?}"),
        }
    }

    fn walk_count(root: &Path) -> usize {
        let mut n = 0;
        for entry in walkdir::WalkDir::new(root)
            .into_iter()
            .filter_map(Result::ok)
        {
            if entry.file_type().is_file() {
                n += 1;
            }
        }
        n
    }
}

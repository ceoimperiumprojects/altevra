//! E2 — prompt self-tweaking (trust-laddered).
//!
//! Altevra proposes improvements to its OWN resident-mode prompts
//! (`06-skills/resident-agent-modes/*.md`) and routes them through the review
//! queue. A `prompt_tweak` is **NEVER auto-applied** — it sits at `proposed`
//! until Pavle approves via `altevra prompt tweaks approve`, which then applies
//! the carried UNIFIED DIFF to the MANAGED REGION of the prompt file via the R5
//! block-level guarded writer (manifest + backup + drift-refuse), versioned and
//! reversible.
//!
//! This builds on the EXISTING promotion machinery — it does NOT fork a parallel
//! pipeline:
//!   * proposals live in the unified `proposals` table (kind = `prompt_tweak`);
//!   * the guarded apply is [`altevra_skills::block_writer::write_block`];
//!   * the diff is applied by [`altevra_skills::unified_diff::apply_unified_diff`];
//!   * a rejected tweak is fingerprinted in `skillopt_meta` so the same diff is
//!     never re-proposed (the SI-13 dedup_hash handles re-proposal at propose
//!     time; the fingerprint memory is the belt for a DIFFERENT signal source
//!     re-deriving the same edit).
//!
//! ## Why `prompt_tweak` is a distinct kind from `prompt`
//!
//! The existing `prompt` kind drives the SI-10 *registry* self-activate path
//! (a DB-stored prompt body with a shadow-eval gate). A `prompt_tweak` edits a
//! human-readable mode prompt FILE on disk through the block writer — a
//! different surface with a different (review-only, never-auto) trust rule. The
//! firewall already treats unknown kinds as ≥ Tier-1 (so even if a tweak leaked
//! into the auto-apply path, it would be denied); the proposal `risk_tier` is
//! re-derived from the kind by the repo (SI-9), and we additionally assert the
//! review-only rule in code here.

use altevra_db::{
    NewProposal, ProposalRow, ProposalsRepository, ReviewItemRow, SkilloptMetaRepository,
    TasksRepository,
};
use altevra_skills::block_writer::{self, WriteOutcome};
use altevra_skills::unified_diff::{apply_unified_diff, validate_unified_diff};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};

/// The proposal kind for a prompt self-tweak. Distinct from `prompt` (the SI-10
/// registry path) — this targets a mode prompt FILE through the block writer.
pub const PROMPT_TWEAK_KIND: &str = "prompt_tweak";

/// The marker label for the managed region a prompt_tweak writes into. The human
/// author owns everything OUTSIDE these markers; the tweak only ever edits the
/// bytes between them.
pub const PROMPT_TWEAK_MARKER: &str = "altevra-prompt-tweak";

/// Structured body carried by a `prompt_tweak` proposal (stored as JSON in the
/// proposals `body` column, mirroring how `skillopt` proposals carry JSON).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PromptTweakBody {
    /// The resident mode being tweaked (e.g. `observer`, `memory_curator`).
    pub mode: String,
    /// Absolute path to the prompt file the diff applies to. Provenance + the
    /// concrete write target. (Resolved by the proposer against the vault; tests
    /// pass a fixture path.)
    pub target_file: String,
    /// The UNIFIED DIFF, applied to the MANAGED REGION body (not the whole file).
    pub diff: String,
    /// Human reason the tweak was proposed (low-quality output, repeated
    /// corrections, or a manual `--reason`).
    pub reason: String,
    /// Order-independent fingerprint of the diff → the reject memory key.
    pub fingerprint: String,
}

/// Build a stable fingerprint of a diff against a mode. A rejected (mode,
/// fingerprint) is never re-proposed. Whitespace-normalized so cosmetically
/// different but semantically identical diffs collapse.
pub fn fingerprint_tweak(mode: &str, diff: &str) -> String {
    let normalized: String = diff
        .lines()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n");
    let raw = format!("{mode}\x00{normalized}");
    block_writer::sha256_hex(&raw)
}

/// Outcome of proposing a prompt tweak.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProposeOutcome {
    /// A new `prompt_tweak` proposal row was created (status `proposed`).
    Proposed(String),
    /// The (mode, fingerprint) was previously REJECTED → refused, no row.
    RefusedRejected,
    /// The same diff was already proposed (dedup merge) → no new row.
    AlreadyProposed(String),
    /// The diff is malformed or does not apply cleanly to the current managed
    /// region → refused at propose time (a bad diff never reaches review).
    RefusedInvalidDiff(String),
}

/// Propose a prompt tweak: validate the diff against the CURRENT managed region
/// of the target file, refuse if it was previously rejected, then insert a
/// `prompt_tweak` proposal (status `proposed`, NEVER auto-applied).
///
/// `target_file` is the concrete prompt file path. The diff applies to the
/// MANAGED REGION body (the text Altevra owns inside its markers). If the file
/// has no managed region yet, the diff applies against the EMPTY managed body
/// (i.e. a pure-insertion diff seeds the region) — the human-authored prompt
/// outside the markers is never the diff's subject.
pub async fn propose_prompt_tweak(
    pool: &SqlitePool,
    mode: &str,
    target_file: &Path,
    diff: &str,
    reason: &str,
) -> anyhow::Result<ProposeOutcome> {
    let fingerprint = fingerprint_tweak(mode, diff);

    // Reject memory: a previously-rejected tweak is never re-proposed.
    let meta = SkilloptMetaRepository::new(pool);
    let meta_key = format!("prompt_tweak:{mode}");
    if meta.was_tried(&meta_key, &fingerprint).await? {
        // `was_tried` is true for any recorded outcome; only a `rejected` outcome
        // should block. Inspect the rows to be precise.
        let rows = meta.list_for_skill(&meta_key).await?;
        if rows
            .iter()
            .any(|r| r.fingerprint == fingerprint && r.outcome == "rejected")
        {
            return Ok(ProposeOutcome::RefusedRejected);
        }
    }

    // Validate the diff against the CURRENT managed region body. A diff that does
    // not parse or does not apply cleanly is refused here — never queued.
    let current_body = read_managed_body(target_file);
    if let Err(e) = validate_unified_diff(&current_body, diff) {
        return Ok(ProposeOutcome::RefusedInvalidDiff(e.to_string()));
    }

    // Guard the reason text before it lands durable.
    let guarded =
        altevra_secrets::guard_text(reason, altevra_core::security::Sensitivity::Internal);
    let reason = guarded.value;

    let body = PromptTweakBody {
        mode: mode.to_string(),
        target_file: target_file.display().to_string(),
        diff: diff.to_string(),
        reason: reason.clone(),
        fingerprint: fingerprint.clone(),
    };

    let proposals = ProposalsRepository::new(pool);
    let (id, is_new) = proposals
        .insert(&NewProposal {
            kind: PROMPT_TWEAK_KIND.into(),
            title: format!("prompt tweak: resident mode '{mode}'"),
            body: serde_json::to_string(&body)?,
            source_mode: Some("self_improve".into()),
            // SI-13 dedup: same mode + diff fingerprint merges, never a 2nd row.
            dedup_hash: format!("prompt_tweak:{mode}:{fingerprint}"),
            evidence_refs: vec![format!("prompt_file:{}", target_file.display())],
            // A prompt_tweak edits an Altevra-owned managed region — not a
            // constitutional/sensitive surface. The review-only rule is enforced
            // by the apply path, not by tier inflation.
            touches_sensitive: false,
            touches_constitutional: false,
        })
        .await?;

    if is_new {
        Ok(ProposeOutcome::Proposed(id))
    } else {
        Ok(ProposeOutcome::AlreadyProposed(id))
    }
}

/// Read the current MANAGED REGION body of a prompt file. Returns the empty
/// string when the file is absent or has no managed region yet (a tweak then
/// applies against an empty body — a pure-insertion seed).
pub fn read_managed_body(target_file: &Path) -> String {
    let Ok(content) = std::fs::read_to_string(target_file) else {
        return String::new();
    };
    match block_writer::parse_block(&content) {
        block_writer::ParseResult::Found { block_bytes, .. } => {
            // Strip the marker lines — the diff applies to the INNER body only.
            strip_markers(&block_bytes)
        }
        _ => String::new(),
    }
}

/// Strip the START/END marker lines from a parsed block, leaving the inner body.
fn strip_markers(block_bytes: &str) -> String {
    let mut lines: Vec<&str> = block_bytes.lines().collect();
    if !lines.is_empty()
        && lines[0].contains(block_writer::MARKER_START)
    {
        lines.remove(0);
    }
    if !lines.is_empty()
        && lines[lines.len() - 1].contains(block_writer::MARKER_END)
    {
        lines.pop();
    }
    let mut body = lines.join("\n");
    if !body.is_empty() {
        body.push('\n');
    }
    body
}

/// The result of approving (applying) a prompt tweak.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyTweakOutcome {
    /// The diff was applied into the managed region; the file was written.
    Applied { new_block_hash: String },
    /// The block writer detected DRIFT (a human edited the managed region since
    /// Altevra last wrote it) → refused, a review item was filed, file untouched.
    DriftRefused,
    /// The diff no longer applies to the current managed region (the file
    /// changed) → refused, file untouched.
    DiffNoLongerApplies(String),
    /// The block writer refused (duplicate/nested markers) → file untouched.
    WriterRefused(String),
}

/// Apply an APPROVED prompt tweak to the managed region of its target file via
/// the R5 block-level guarded writer. Backup + manifest + drift-refuse all come
/// from the writer; we additionally re-apply the diff to the LIVE managed body
/// (not a stale snapshot) so an approval that races a file change refuses
/// cleanly rather than clobbering.
///
/// This is ONLY called from the human-gated approve path — never from the
/// self-improve loop.
pub async fn apply_prompt_tweak(
    pool: &SqlitePool,
    body: &PromptTweakBody,
    backup_root: &Path,
    apply: bool,
) -> anyhow::Result<ApplyTweakOutcome> {
    let target = PathBuf::from(&body.target_file);

    // Re-derive the new managed body from the LIVE current body (defends against
    // a file change between propose and approve).
    let current_body = read_managed_body(&target);
    let new_body = match apply_unified_diff(&current_body, &body.diff) {
        Ok(b) => b,
        Err(e) => return Ok(ApplyTweakOutcome::DiffNoLongerApplies(e.to_string())),
    };

    // Drift baseline = the manifest hash for (file, marker). The writer refuses
    // if the on-disk managed block differs from what Altevra last wrote.
    let bw_repo = altevra_db::BlockWritesRepository::new(pool);
    let file_key = target.display().to_string();
    let existing_manifest = bw_repo.get(&file_key, PROMPT_TWEAK_MARKER).await?;
    let manifest_hash = existing_manifest.as_ref().map(|r| r.block_hash.as_str());

    let (outcome, new_hash) = block_writer::write_block(
        &target,
        &new_body,
        PROMPT_TWEAK_MARKER,
        manifest_hash,
        apply,
    )?;

    match outcome {
        WriteOutcome::Drift { .. } => {
            // File a review item so Pavle can decide (mirrors memory-sync drift).
            let item = ReviewItemRow {
                id: uuid::Uuid::new_v4(),
                project_id: None,
                kind: "prompt_tweak_drift".into(),
                title: format!("prompt-tweak drift: managed block in {}", body.mode),
                body: Some(format!(
                    "The ALTEVRA_MANAGED prompt-tweak block in '{}' was edited since \
                     Altevra last wrote it. The tweak apply was REFUSED to protect the \
                     human edit.",
                    target.display()
                )),
                status: "open".into(),
                created_at: Utc::now(),
                metadata: serde_json::json!({
                    "target_file": file_key,
                    "mode": body.mode,
                    "marker_id": PROMPT_TWEAK_MARKER,
                }),
            };
            let _ = TasksRepository::new(pool).create_review_item(&item).await;
            Ok(ApplyTweakOutcome::DriftRefused)
        }
        WriteOutcome::Refused(reason) => Ok(ApplyTweakOutcome::WriterRefused(reason)),
        WriteOutcome::Appended | WriteOutcome::Refreshed | WriteOutcome::AlreadyInSync => {
            let hash = new_hash.unwrap_or_default();
            if apply {
                // Record a backup-path + manifest baseline (versioned + reversible).
                let backup_path = backup_root
                    .join(Utc::now().format("%Y%m%dT%H%M%S").to_string())
                    .join(
                        target
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| "prompt.md".into()),
                    );
                let _ = bw_repo
                    .record_write(
                        &file_key,
                        PROMPT_TWEAK_MARKER,
                        &hash,
                        Some(&backup_path.display().to_string()),
                        Some(
                            &serde_json::json!({
                                "mode": body.mode,
                                "source": "prompt_tweak_approve",
                                "ingest_ts": Utc::now().to_rfc3339(),
                            })
                            .to_string(),
                        ),
                    )
                    .await;
            }
            Ok(ApplyTweakOutcome::Applied {
                new_block_hash: hash,
            })
        }
    }
}

/// Record a rejected tweak's fingerprint so the same (mode, diff) is never
/// re-proposed (skillopt_meta-style, `outcome = "rejected"`).
pub async fn record_rejected(
    pool: &SqlitePool,
    body: &PromptTweakBody,
    reason: &str,
) -> anyhow::Result<()> {
    let meta = SkilloptMetaRepository::new(pool);
    let meta_key = format!("prompt_tweak:{}", body.mode);
    meta.record_tried(
        &meta_key,
        &body.fingerprint,
        &serde_json::json!({ "reject_reason": reason, "mode": body.mode }),
        "rejected",
    )
    .await
}

// ---------------------------------------------------------------------------
// Heuristic signal source — repeated prompt-targeting proposals / corrections.
// ---------------------------------------------------------------------------

/// A lightweight heuristic that scans recent improvement proposals for SIGNS a
/// resident mode is producing low-quality output (≥ [`CORRECTION_THRESHOLD`]
/// open `improvement`/`prompt` proposals whose `source_mode` is one mode), and
/// returns the mode names worth a prompt tweak. This is the "start simple"
/// detector the spec calls for — it surfaces candidates; it NEVER auto-proposes
/// a diff (a human authors the diff, or a later LLM polish path does, both
/// through the review queue).
pub const CORRECTION_THRESHOLD: usize = 3;

/// Scan open proposals; return modes with ≥ threshold open improvement signals
/// attributed to them (sorted by count desc).
pub async fn detect_low_quality_modes(pool: &SqlitePool) -> anyhow::Result<Vec<(String, usize)>> {
    let proposals = ProposalsRepository::new(pool);
    let open = proposals.list(Some("proposed"), None).await?;

    use std::collections::HashMap;
    let mut counts: HashMap<String, usize> = HashMap::new();
    for p in &open {
        // Only count signals that point AT a resident mode's quality.
        if p.kind != "improvement" && p.kind != "prompt" {
            continue;
        }
        if let Some(mode) = &p.source_mode {
            // The orchestrator's own meta-modes are not tweak targets.
            if mode == "self_improve" {
                continue;
            }
            *counts.entry(mode.clone()).or_default() += 1;
        }
    }

    let mut out: Vec<(String, usize)> = counts
        .into_iter()
        .filter(|(_, c)| *c >= CORRECTION_THRESHOLD)
        .collect();
    out.sort_by(|a, b| b.1.cmp(&a.1));
    Ok(out)
}

/// Parse a `prompt_tweak` proposal row's JSON body back into [`PromptTweakBody`].
pub fn parse_tweak_body(row: &ProposalRow) -> anyhow::Result<PromptTweakBody> {
    Ok(serde_json::from_str(&row.body)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use altevra_db::{create_pool, run_migrations};
    use altevra_skills::block_writer::wrap_block;
    use tempfile::TempDir;

    async fn pool() -> SqlitePool {
        let p = create_pool("sqlite::memory:").await.unwrap();
        run_migrations(&p).await.unwrap();
        p
    }

    /// A prompt fixture: human-authored prose OUTSIDE the markers + a managed
    /// region with one tweakable line INSIDE.
    fn write_prompt_fixture(dir: &TempDir, inner_body: &str) -> PathBuf {
        let block = wrap_block(inner_body, PROMPT_TWEAK_MARKER, block_writer::Eol::Lf);
        let content = format!(
            "# Mode: Observer\n\nHUMAN AUTHORED PROSE — never touched.\n\n{block}\nMORE HUMAN PROSE.\n"
        );
        let p = dir.path().join("observer.md");
        std::fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn fingerprint_is_whitespace_stable() {
        let a = fingerprint_tweak("observer", "@@ -1,1 +1,1 @@\n-a\n+b\n");
        let b = fingerprint_tweak("observer", "@@ -1,1 +1,1 @@\n-a   \n+b\n");
        assert_eq!(a, b, "trailing whitespace must not change the fingerprint");
        let c = fingerprint_tweak("synthesis", "@@ -1,1 +1,1 @@\n-a\n+b\n");
        assert_ne!(a, c, "mode is part of the fingerprint");
    }

    #[tokio::test]
    async fn propose_then_refuse_invalid_diff() {
        let p = pool().await;
        let tmp = TempDir::new().unwrap();
        let f = write_prompt_fixture(&tmp, "old guidance line\n");

        // A diff that does not match the managed body → refused at propose.
        let bad = "@@ -1,1 +1,1 @@\n-NONEXISTENT LINE\n+new\n";
        let out = propose_prompt_tweak(&p, "observer", &f, bad, "test").await.unwrap();
        assert!(
            matches!(out, ProposeOutcome::RefusedInvalidDiff(_)),
            "{out:?}"
        );
        // Nothing queued.
        let rows = ProposalsRepository::new(&p)
            .list(None, Some(PROMPT_TWEAK_KIND))
            .await
            .unwrap();
        assert!(rows.is_empty());
    }

    #[tokio::test]
    async fn propose_lifecycle_apply_preserves_human_bytes() {
        let p = pool().await;
        let tmp = TempDir::new().unwrap();
        let f = write_prompt_fixture(&tmp, "old guidance line\n");
        let before = std::fs::read_to_string(&f).unwrap();

        // A valid diff editing the managed body's single line.
        let diff = "@@ -1,1 +1,1 @@\n-old guidance line\n+new guidance line\n";
        let out = propose_prompt_tweak(&p, "observer", &f, diff, "low-quality output")
            .await
            .unwrap();
        let id = match out {
            ProposeOutcome::Proposed(id) => id,
            other => panic!("expected Proposed, got {other:?}"),
        };

        // List → show.
        let row = ProposalsRepository::new(&p).get(&id).await.unwrap().unwrap();
        assert_eq!(row.kind, PROMPT_TWEAK_KIND);
        assert_eq!(row.status, "proposed");
        let body = parse_tweak_body(&row).unwrap();
        assert_eq!(body.mode, "observer");

        // Apply via the guarded writer.
        let backup_root = tmp.path().join("backups");
        let applied = apply_prompt_tweak(&p, &body, &backup_root, true).await.unwrap();
        assert!(matches!(applied, ApplyTweakOutcome::Applied { .. }), "{applied:?}");

        // The managed region changed; the HUMAN bytes outside the markers survive.
        let after = std::fs::read_to_string(&f).unwrap();
        assert!(after.contains("new guidance line"), "managed region updated");
        assert!(!after.contains("old guidance line"), "old line replaced");
        assert!(
            after.contains("# Mode: Observer\n\nHUMAN AUTHORED PROSE — never touched."),
            "human prefix preserved byte-identically"
        );
        assert!(after.contains("MORE HUMAN PROSE."), "human suffix preserved");
        // And the human content is byte-identical to before, outside the block.
        let before_prefix = before.split("<!-- ALTEVRA_MANAGED_START").next().unwrap();
        let after_prefix = after.split("<!-- ALTEVRA_MANAGED_START").next().unwrap();
        assert_eq!(before_prefix, after_prefix, "prefix bytes identical");

        // A manifest row was recorded (versioned + reversible).
        let manifest = altevra_db::BlockWritesRepository::new(&p)
            .get(&f.display().to_string(), PROMPT_TWEAK_MARKER)
            .await
            .unwrap();
        assert!(manifest.is_some(), "manifest baseline recorded after apply");
        assert!(manifest.unwrap().backup_path.is_some(), "backup path recorded");
    }

    #[tokio::test]
    async fn reject_records_fingerprint_and_re_propose_refused() {
        let p = pool().await;
        let tmp = TempDir::new().unwrap();
        let f = write_prompt_fixture(&tmp, "guidance\n");
        let diff = "@@ -1,1 +1,1 @@\n-guidance\n+better guidance\n";

        // Propose, then reject (record fingerprint).
        let out = propose_prompt_tweak(&p, "observer", &f, diff, "r").await.unwrap();
        let id = match out {
            ProposeOutcome::Proposed(id) => id,
            other => panic!("{other:?}"),
        };
        let row = ProposalsRepository::new(&p).get(&id).await.unwrap().unwrap();
        let body = parse_tweak_body(&row).unwrap();
        record_rejected(&p, &body, "not better").await.unwrap();

        // Re-proposing the SAME (mode, diff) is refused.
        let again = propose_prompt_tweak(&p, "observer", &f, diff, "r2").await.unwrap();
        assert_eq!(again, ProposeOutcome::RefusedRejected);
    }

    #[tokio::test]
    async fn detect_low_quality_modes_threshold() {
        let p = pool().await;
        let repo = ProposalsRepository::new(&p);
        // 3 improvement proposals attributed to "synthesis" → over threshold.
        for i in 0..3 {
            repo.insert(&NewProposal {
                kind: "improvement".into(),
                title: format!("low quality {i}"),
                body: "b".into(),
                source_mode: Some("synthesis".into()),
                dedup_hash: format!("lq:{i}"),
                evidence_refs: vec![],
                touches_sensitive: false,
                touches_constitutional: false,
            })
            .await
            .unwrap();
        }
        // 1 for "observer" → under threshold.
        repo.insert(&NewProposal {
            kind: "improvement".into(),
            title: "one".into(),
            body: "b".into(),
            source_mode: Some("observer".into()),
            dedup_hash: "obs:1".into(),
            evidence_refs: vec![],
            touches_sensitive: false,
            touches_constitutional: false,
        })
        .await
        .unwrap();

        let modes = detect_low_quality_modes(&p).await.unwrap();
        assert_eq!(modes.len(), 1);
        assert_eq!(modes[0].0, "synthesis");
        assert_eq!(modes[0].1, 3);
    }

    #[tokio::test]
    async fn apply_refuses_on_drift() {
        let p = pool().await;
        let tmp = TempDir::new().unwrap();
        let f = write_prompt_fixture(&tmp, "guidance\n");
        let diff = "@@ -1,1 +1,1 @@\n-guidance\n+new guidance\n";

        let out = propose_prompt_tweak(&p, "observer", &f, diff, "r").await.unwrap();
        let id = match out {
            ProposeOutcome::Proposed(id) => id,
            other => panic!("{other:?}"),
        };
        let body = parse_tweak_body(
            &ProposalsRepository::new(&p).get(&id).await.unwrap().unwrap(),
        )
        .unwrap();
        let backup_root = tmp.path().join("backups");

        // First apply succeeds (records the manifest baseline).
        apply_prompt_tweak(&p, &body, &backup_root, true).await.unwrap();

        // A human edits INSIDE the managed region → drift baseline mismatch.
        let content = std::fs::read_to_string(&f).unwrap();
        let tampered = content.replace("new guidance", "HUMAN EDIT inside managed region");
        std::fs::write(&f, &tampered).unwrap();

        // Re-applying a (now stale) tweak: the diff no longer applies to the live
        // body (it sought "guidance"), so we refuse with DiffNoLongerApplies — the
        // file is left byte-identical. (Both DiffNoLongerApplies and DriftRefused
        // are non-clobbering refusals; this fixture trips the diff-mismatch first.)
        let re = apply_prompt_tweak(&p, &body, &backup_root, true).await.unwrap();
        assert!(
            matches!(re, ApplyTweakOutcome::DiffNoLongerApplies(_) | ApplyTweakOutcome::DriftRefused),
            "{re:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&f).unwrap(),
            tampered,
            "a refused apply must leave the human edit byte-identical"
        );
    }
}

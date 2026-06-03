//! Prompt registry-of-record + self-modify gate (working draft §4.8, R10).
//!
//! [`prompts.rs`](crate::prompts) is a STATELESS layered *builder* — it satisfies
//! neither SI-8 (exactly one active row per slug) nor SI-2 (constitutional lock).
//! This module is the *registry*: the pure, DB-free decision core that sits BELOW
//! the LLM and governs how Altevra rewrites its OWN prompts.
//!
//! Like the [`firewall`](crate::selfimprove::firewall), every rule here is a
//! function of STRUCTURED fields (name/version/layer/locked/active/checksum) — never
//! free text — so no note or proposal CONTENT can flip a verdict (SI-15). The
//! aggressive-autonomy mode changes only HOW MUCH auto-applies; the brakes encoded
//! here can NOT be removed by anything the loop emits.
//!
//! Persistence (the actual `prompts` table + the SI-8 deactivate-old-then-activate-new
//! transaction) lives in `altevra-db`'s `PromptsRepository`, which calls into the
//! pure decisions here. Core stays DB-free (no sqlx), mirroring `derive_risk_tier`
//! / `firewall_check`.
//!
//! Invariants this module owns:
//!  - **SI-8 (one active per slug):** [`mint_plan`] emits a plan that deactivates
//!    every prior active row for the name before activating the new one; the repo
//!    runs it in a single transaction. [`assert_one_active_per_slug`] validates a
//!    snapshot.
//!  - **SI-2 (constitutional lock):** a row whose `locked = 1` (safety,
//!    altevra_rules) can NOT be deactivated/replaced through the mint path —
//!    [`mint_plan`] returns [`MintError::ConstitutionalLock`] pointing at the
//!    Tier-2 presence path. Aggressive mode does not bypass this.
//!  - **SI-10 (shadow-eval gate):** [`try_auto_activate`] auto-activates a
//!    NON-locked candidate ONLY when a passing [`PromptEval`] exists; a regression
//!    auto-rejects; no eval keeps it inactive (proposed).
//!  - **Checksum-drift:** [`detect_drift`] flags a hand-edited generated prompt
//!    (stored checksum ≠ recomputed body checksum) — it is surfaced, never
//!    silently overwritten.

use std::collections::BTreeMap;
use std::fmt;

/// A prompt registry record — a STRUCTURED mirror of one `prompts` table row
/// (migration 028: `name, version, layer, body, locked, active`). The registry
/// decisions read only these fields; there is no free-text path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptRecord {
    /// Slug (`safety`, `altevra_rules`, `resident_mode:observer`, …). Exactly one
    /// active row per `name` (SI-8).
    pub name: String,
    pub version: i64,
    pub layer: String,
    pub body: String,
    /// `true` = constitutional (safety / altevra_rules) — never auto-changed (SI-2).
    pub locked: bool,
    pub active: bool,
}

impl PromptRecord {
    /// The content checksum of this record's body. Byte-deterministic and stable
    /// across processes (FNV-1a 64 over the body bytes), so a render manifest can
    /// be replayed and a hand-edit can be detected as drift.
    pub fn checksum(&self) -> String {
        checksum_body(&self.body)
    }
}

/// FNV-1a 64-bit over the raw body bytes, lowercase hex. Process-independent
/// (unlike `DefaultHasher`, whose seed is randomized) so render manifests replay
/// byte-for-byte and stored checksums survive restarts.
pub fn checksum_body(body: &str) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = OFFSET;
    for b in body.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(PRIME);
    }
    format!("{h:016x}")
}

// ---------------------------------------------------------------------------
// SI-8 — exactly one active row per slug
// ---------------------------------------------------------------------------

/// A snapshot invariant violation: more than one active row for some slug.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateActive {
    pub name: String,
    pub active_versions: Vec<i64>,
}

/// SI-8 validator: assert at most one active row per `name` in a snapshot. The
/// repo upholds this via a transaction; this is the read-side check (tests +
/// drift audits). Returns the offending slug(s), if any.
pub fn assert_one_active_per_slug(snapshot: &[PromptRecord]) -> Result<(), Vec<DuplicateActive>> {
    let mut active_by_name: BTreeMap<&str, Vec<i64>> = BTreeMap::new();
    for r in snapshot.iter().filter(|r| r.active) {
        active_by_name
            .entry(r.name.as_str())
            .or_default()
            .push(r.version);
    }
    let dups: Vec<DuplicateActive> = active_by_name
        .into_iter()
        .filter(|(_, v)| v.len() > 1)
        .map(|(name, mut active_versions)| {
            active_versions.sort_unstable();
            DuplicateActive {
                name: name.to_string(),
                active_versions,
            }
        })
        .collect();
    if dups.is_empty() {
        Ok(())
    } else {
        Err(dups)
    }
}

// ---------------------------------------------------------------------------
// Mint plan (SI-8 transaction shape + SI-2 constitutional lock)
// ---------------------------------------------------------------------------

/// Why a mint was refused. SI-2's [`MintError::ConstitutionalLock`] points at the
/// Tier-2 presence path — the only way a locked layer ever changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MintError {
    /// SI-2: the slug has a constitutional-locked row. The normal mint path may
    /// NOT deactivate/replace it. (The message names the Tier-2 escape hatch.)
    ConstitutionalLock { name: String },
    /// The candidate version already exists in the snapshot (would collide).
    VersionExists { name: String, version: i64 },
    /// The candidate version is not greater than the current max (monotonic mint).
    NonMonotonic {
        name: String,
        version: i64,
        current_max: i64,
    },
}

impl fmt::Display for MintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MintError::ConstitutionalLock { name } => write!(
                f,
                "constitutional_lock: prompt '{name}' is locked (Tier-2). The normal \
                 mint path can NOT deactivate or replace it. Use the Tier-2 \
                 presence path (`altevra control review` + human presence); \
                 aggressive autonomy does not bypass this."
            ),
            MintError::VersionExists { name, version } => {
                write!(f, "version_exists: prompt '{name}' v{version} already exists")
            }
            MintError::NonMonotonic {
                name,
                version,
                current_max,
            } => write!(
                f,
                "non_monotonic: prompt '{name}' candidate v{version} must be greater \
                 than current max v{current_max}"
            ),
        }
    }
}

impl std::error::Error for MintError {}

/// The deterministic plan to make a new version the single active row for a slug,
/// upholding SI-8. The repo executes this in ONE transaction:
///   1. insert `(name, version, layer, body, locked=0, active=0)`
///   2. set `active = 0` for every `deactivate_versions`
///   3. set `active = 1` for `activate_version` (only when [`activate_now`] is true)
///
/// When `activate_now` is false (SI-10: no passing shadow eval yet) the new row is
/// inserted INACTIVE — a proposal — and no prior row is touched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintPlan {
    pub name: String,
    pub layer: String,
    pub body: String,
    pub new_version: i64,
    /// Versions whose `active` flag the repo must clear (only when activating).
    pub deactivate_versions: Vec<i64>,
    pub activate_version: i64,
    /// Whether the new version becomes active immediately. False → proposed-only.
    pub activate_now: bool,
}

/// Build the SI-8 mint plan for `(name, new_version)` against a snapshot.
///
/// * SI-2: if ANY row for `name` is `locked`, refuse with
///   [`MintError::ConstitutionalLock`] — aggressive mode does not bypass it.
/// * Monotonic: `new_version` must be strictly greater than the current max for
///   `name` and must not already exist.
/// * `activate_now` is threaded through to the plan: when false the new row is a
///   proposed-only (inactive) insert and NO prior active row is deactivated.
pub fn mint_plan(
    snapshot: &[PromptRecord],
    name: &str,
    new_version: i64,
    layer: &str,
    body: &str,
    activate_now: bool,
) -> Result<MintPlan, MintError> {
    // SI-2: a locked slug is constitutional — refuse the normal mint path.
    if snapshot.iter().any(|r| r.name == name && r.locked) {
        return Err(MintError::ConstitutionalLock {
            name: name.to_string(),
        });
    }

    let mut current_max: Option<i64> = None;
    for r in snapshot.iter().filter(|r| r.name == name) {
        if r.version == new_version {
            return Err(MintError::VersionExists {
                name: name.to_string(),
                version: new_version,
            });
        }
        current_max = Some(current_max.map_or(r.version, |m| m.max(r.version)));
    }
    if let Some(m) = current_max {
        if new_version <= m {
            return Err(MintError::NonMonotonic {
                name: name.to_string(),
                version: new_version,
                current_max: m,
            });
        }
    }

    // When activating, every currently-active row for this slug is deactivated
    // (SI-8). When proposing-only, nothing is touched.
    let deactivate_versions = if activate_now {
        snapshot
            .iter()
            .filter(|r| r.name == name && r.active)
            .map(|r| r.version)
            .collect()
    } else {
        Vec::new()
    };

    Ok(MintPlan {
        name: name.to_string(),
        layer: layer.to_string(),
        body: body.to_string(),
        new_version,
        deactivate_versions,
        activate_version: new_version,
        activate_now,
    })
}

// ---------------------------------------------------------------------------
// SI-10 — shadow-eval gate
// ---------------------------------------------------------------------------

/// A shadow A/B eval result for a candidate prompt version (mirror of one
/// `prompt_eval_results` row). STRUCTURED only — the gate reads `passed` +
/// `score_delta`, never free text.
#[derive(Debug, Clone, PartialEq)]
pub struct PromptEval {
    pub prompt_name: String,
    pub candidate_version: i64,
    pub baseline_version: i64,
    /// candidate − baseline; a negative delta is a regression.
    pub score_delta: f64,
    pub passed: bool,
}

/// The SI-10 gate decision for whether a NON-locked candidate may auto-activate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutoActivateDecision {
    /// A passing eval exists → the orchestrator may run the activate transaction.
    Activate,
    /// A regression (eval present but not passing / negative delta) → auto-reject.
    AutoReject { reason: String },
    /// No eval yet → stays inactive (proposed).
    StayProposed { reason: String },
    /// The slug is constitutional-locked (SI-2) → never auto-activate.
    ConstitutionalLock { name: String },
}

impl AutoActivateDecision {
    pub fn is_activate(&self) -> bool {
        matches!(self, AutoActivateDecision::Activate)
    }
}

/// SI-10 self-modify gate: decide whether `(name, candidate_version)` may
/// auto-activate, given the snapshot and the candidate's eval (if any).
///
/// Order is most-protective first:
///  1. SI-2 — a locked slug never auto-activates (constitutional).
///  2. No eval row → [`AutoActivateDecision::StayProposed`] (inactive proposal).
///  3. Eval present but `passed == false` OR `score_delta < 0` → auto-reject.
///  4. Eval present, passing, non-regressive → activate.
///
/// This is the method the orchestrator (C2) calls; it returns a DECISION only —
/// the repo runs the transaction when (and only when) the decision is `Activate`.
pub fn try_auto_activate(
    snapshot: &[PromptRecord],
    name: &str,
    candidate_version: i64,
    eval: Option<&PromptEval>,
) -> AutoActivateDecision {
    // 1. SI-2: locked slug → never auto-activate.
    if snapshot.iter().any(|r| r.name == name && r.locked) {
        return AutoActivateDecision::ConstitutionalLock {
            name: name.to_string(),
        };
    }
    match eval {
        // 2. No shadow eval ran → proposed-only.
        None => AutoActivateDecision::StayProposed {
            reason: format!(
                "no shadow eval for '{name}' v{candidate_version}; stays inactive (proposed)"
            ),
        },
        Some(e) => {
            // Defend against a mismatched eval being passed in.
            if e.prompt_name != name || e.candidate_version != candidate_version {
                return AutoActivateDecision::StayProposed {
                    reason: format!(
                        "eval is for {}:v{} not {}:v{}; stays inactive",
                        e.prompt_name, e.candidate_version, name, candidate_version
                    ),
                };
            }
            // 3. A regression (not passing, or a negative delta) → auto-reject.
            if !e.passed || e.score_delta < 0.0 {
                AutoActivateDecision::AutoReject {
                    reason: format!(
                        "shadow eval regression for '{name}' v{candidate_version} \
                         (passed={}, score_delta={:+.4}); auto-rejected",
                        e.passed, e.score_delta
                    ),
                }
            } else {
                // 4. Passing, non-regressive → activate.
                AutoActivateDecision::Activate
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Deterministic render + manifest
// ---------------------------------------------------------------------------

/// One entry in a [`RenderManifest`]: which version+checksum of a slug was
/// composed. Replayable — the same snapshot reproduces the same manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderManifestEntry {
    pub name: String,
    pub version: i64,
    pub checksum: String,
}

/// A render manifest: the ordered list of `(slug → version → checksum)` that were
/// composed into a prompt, for replay + audit.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RenderManifest {
    pub entries: Vec<RenderManifestEntry>,
}

/// A slug requested for render but with no active row in the snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingSlug {
    pub name: String,
}

/// The deterministic render output: the composed prompt + its manifest + any
/// requested-but-missing slugs (surfaced, never silently dropped).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedPrompt {
    pub prompt: String,
    pub manifest: RenderManifest,
    pub missing: Vec<MissingSlug>,
}

/// Pure, byte-deterministic render: compose the ACTIVE rows for `slug_set` (in the
/// caller's given order) from `snapshot`, substituting `{{var}}` placeholders from
/// `variables`, and emit a [`RenderManifest`] (slug → version → checksum).
///
/// Determinism guarantees (so the manifest replays byte-for-byte):
///  - the only rows consulted are `active` rows whose `name` is in `slug_set`;
///  - slugs render in the order given (no map iteration);
///  - variable substitution is a fixed left-to-right pass over a sorted key list;
///  - the checksum is FNV-1a (process-independent), computed on the FINAL
///    (post-substitution) body so the manifest reflects exactly what was emitted.
///
/// A requested slug with no active row is recorded in `missing` (not an error) so
/// the caller can decide; a duplicate-active snapshot is the repo's SI-8 problem
/// (validate with [`assert_one_active_per_slug`]) — here we deterministically take
/// the lowest version to stay total.
pub fn render(
    snapshot: &[PromptRecord],
    slug_set: &[&str],
    variables: &BTreeMap<String, String>,
) -> RenderedPrompt {
    let mut prompt = String::new();
    let mut entries = Vec::new();
    let mut missing = Vec::new();

    for &slug in slug_set {
        // Active row for this slug; if SI-8 is somehow violated, pick the lowest
        // version deterministically (BTreeMap-free: a min scan).
        let chosen = snapshot
            .iter()
            .filter(|r| r.active && r.name == slug)
            .min_by_key(|r| r.version);
        match chosen {
            Some(rec) => {
                let composed = substitute(&rec.body, variables);
                let checksum = checksum_body(&composed);
                if !prompt.is_empty() {
                    prompt.push_str("\n\n");
                }
                prompt.push_str(&composed);
                entries.push(RenderManifestEntry {
                    name: rec.name.clone(),
                    version: rec.version,
                    checksum,
                });
            }
            None => missing.push(MissingSlug {
                name: slug.to_string(),
            }),
        }
    }

    RenderedPrompt {
        prompt,
        manifest: RenderManifest { entries },
        missing,
    }
}

/// Deterministic `{{key}}` substitution. Keys are applied in sorted order so the
/// result is identical regardless of map insertion order. Unknown placeholders are
/// left untouched (no panics, no nondeterministic defaults).
fn substitute(body: &str, variables: &BTreeMap<String, String>) -> String {
    // `BTreeMap` already iterates in sorted key order — deterministic by type.
    let mut out = body.to_string();
    for (k, v) in variables {
        let needle = format!("{{{{{k}}}}}");
        if out.contains(&needle) {
            out = out.replace(&needle, v);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Checksum-drift detection
// ---------------------------------------------------------------------------

/// A drift finding: a stored checksum disagrees with the recomputed body checksum
/// — i.e. the prompt body was hand-edited out from under the registry. Surfaced,
/// never silently overwritten.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftFinding {
    pub name: String,
    pub version: i64,
    pub stored_checksum: String,
    pub actual_checksum: String,
}

/// Detect checksum drift: for each record, compare a `stored_checksum` (e.g. from
/// a prior render manifest) against the record's recomputed body checksum. Any
/// mismatch is reported. `stored` maps `(name, version) → checksum`.
pub fn detect_drift(
    snapshot: &[PromptRecord],
    stored: &BTreeMap<(String, i64), String>,
) -> Vec<DriftFinding> {
    let mut out = Vec::new();
    for rec in snapshot {
        if let Some(stored_ck) = stored.get(&(rec.name.clone(), rec.version)) {
            let actual = rec.checksum();
            if *stored_ck != actual {
                out.push(DriftFinding {
                    name: rec.name.clone(),
                    version: rec.version,
                    stored_checksum: stored_ck.clone(),
                    actual_checksum: actual,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(name: &str, version: i64, body: &str, locked: bool, active: bool) -> PromptRecord {
        PromptRecord {
            name: name.to_string(),
            version,
            layer: "mode".to_string(),
            body: body.to_string(),
            locked,
            active,
        }
    }

    // ---- SI-8: exactly one active per slug --------------------------------

    #[test]
    fn one_active_per_slug() {
        // Clean snapshot: one active per name → OK.
        let clean = vec![
            rec("safety", 1, "S", true, true),
            rec("observer", 1, "old", false, false),
            rec("observer", 2, "new", false, true),
        ];
        assert!(assert_one_active_per_slug(&clean).is_ok());

        // The mint plan upholds SI-8: activating observer v3 deactivates v2 (the
        // current active), leaving exactly one active after the repo applies it.
        let plan = mint_plan(&clean, "observer", 3, "mode", "newer", true).unwrap();
        assert_eq!(plan.deactivate_versions, vec![2]);
        assert_eq!(plan.activate_version, 3);
        assert!(plan.activate_now);

        // Simulate applying the plan and re-check the invariant holds.
        let mut applied = clean.clone();
        for r in applied.iter_mut() {
            if r.name == "observer" && plan.deactivate_versions.contains(&r.version) {
                r.active = false;
            }
        }
        applied.push(rec("observer", 3, "newer", false, true));
        assert!(assert_one_active_per_slug(&applied).is_ok());

        // A corrupt snapshot with two active rows for one slug is detected.
        let dirty = vec![
            rec("observer", 1, "a", false, true),
            rec("observer", 2, "b", false, true),
        ];
        let err = assert_one_active_per_slug(&dirty).unwrap_err();
        assert_eq!(err.len(), 1);
        assert_eq!(err[0].name, "observer");
        assert_eq!(err[0].active_versions, vec![1, 2]);
    }

    // ---- SI-2: constitutional lock ----------------------------------------

    #[test]
    fn constitutional_locked_cannot_be_replaced_normally() {
        // safety + altevra_rules are seeded locked=1 (migration 028).
        let snapshot = vec![
            rec("safety", 1, "S", true, true),
            rec("altevra_rules", 1, "R", true, true),
        ];
        // The normal mint path refuses to mint a new safety version.
        let err = mint_plan(&snapshot, "safety", 2, "safety", "tampered", true).unwrap_err();
        match err {
            MintError::ConstitutionalLock { ref name } => assert_eq!(name, "safety"),
            other => panic!("expected ConstitutionalLock, got {other:?}"),
        }
        // The error names the Tier-2 presence escape hatch — not a silent failure.
        let msg = err.to_string();
        assert!(msg.contains("Tier-2"));
        assert!(msg.contains("presence"));
        assert!(msg.contains("aggressive autonomy does not bypass"));

        // try_auto_activate ALSO refuses a locked slug (SI-2), even with a passing
        // eval — the aggression knob can not bypass the constitutional lock.
        let passing = PromptEval {
            prompt_name: "altevra_rules".into(),
            candidate_version: 2,
            baseline_version: 1,
            score_delta: 0.9,
            passed: true,
        };
        let d = try_auto_activate(&snapshot, "altevra_rules", 2, Some(&passing));
        assert_eq!(
            d,
            AutoActivateDecision::ConstitutionalLock {
                name: "altevra_rules".into()
            }
        );
        assert!(!d.is_activate());
    }

    // ---- deterministic render ---------------------------------------------

    #[test]
    fn render_is_deterministic() {
        let snapshot = vec![
            rec("safety", 1, "Never leak secrets.", true, true),
            rec("resident:observer", 1, "Observe {{mode}} closely.", false, true),
            rec("resident:observer", 2, "STALE", false, false), // inactive → ignored
        ];
        let mut vars = BTreeMap::new();
        vars.insert("mode".to_string(), "observer".to_string());

        let slug_set = ["safety", "resident:observer"];
        let a = render(&snapshot, &slug_set, &vars);
        let b = render(&snapshot, &slug_set, &vars);

        // Same snapshot → byte-equal prompt AND identical manifest.
        assert_eq!(a.prompt, b.prompt);
        assert_eq!(a.manifest, b.manifest);
        assert_eq!(
            a.prompt,
            "Never leak secrets.\n\nObserve observer closely."
        );
        // Manifest records the ACTIVE versions in request order with checksums of
        // the post-substitution body.
        assert_eq!(a.manifest.entries.len(), 2);
        assert_eq!(a.manifest.entries[0].name, "safety");
        assert_eq!(a.manifest.entries[0].version, 1);
        assert_eq!(
            a.manifest.entries[0].checksum,
            checksum_body("Never leak secrets.")
        );
        assert_eq!(a.manifest.entries[1].name, "resident:observer");
        assert_eq!(a.manifest.entries[1].version, 1);
        assert_eq!(
            a.manifest.entries[1].checksum,
            checksum_body("Observe observer closely.")
        );
        assert!(a.missing.is_empty());

        // A requested-but-missing slug is surfaced, not silently dropped.
        let c = render(&snapshot, &["safety", "no_such_slug"], &vars);
        assert_eq!(c.missing, vec![MissingSlug { name: "no_such_slug".into() }]);
        assert_eq!(c.manifest.entries.len(), 1);
    }

    #[test]
    fn checksum_is_process_independent() {
        // FNV-1a is a fixed function — a known body hashes to a stable value (not a
        // randomized DefaultHasher seed), which is what makes manifests replayable.
        assert_eq!(checksum_body(""), "cbf29ce484222325");
        // Two different bodies differ; the same body is stable.
        assert_ne!(checksum_body("a"), checksum_body("b"));
        assert_eq!(checksum_body("hello"), checksum_body("hello"));
    }

    // ---- SI-10: shadow-eval gate ------------------------------------------

    #[test]
    fn self_modify_requires_passing_shadow_eval() {
        // A non-locked resident-mode prompt, candidate v2.
        let snapshot = vec![rec("resident:observer", 1, "v1", false, true)];

        // No eval → stays proposed (inactive), NOT auto-activated.
        let d_none = try_auto_activate(&snapshot, "resident:observer", 2, None);
        match d_none {
            AutoActivateDecision::StayProposed { .. } => {}
            other => panic!("expected StayProposed, got {other:?}"),
        }
        assert!(!d_none.is_activate());

        // A regression (eval present, not passing) → auto-reject.
        let regressed = PromptEval {
            prompt_name: "resident:observer".into(),
            candidate_version: 2,
            baseline_version: 1,
            score_delta: -0.2,
            passed: false,
        };
        let d_reg = try_auto_activate(&snapshot, "resident:observer", 2, Some(&regressed));
        match d_reg {
            AutoActivateDecision::AutoReject { .. } => {}
            other => panic!("expected AutoReject, got {other:?}"),
        }

        // A negative delta even if `passed` was (wrongly) true is still a regression.
        let neg_delta = PromptEval {
            score_delta: -0.01,
            passed: true,
            ..regressed.clone()
        };
        assert!(matches!(
            try_auto_activate(&snapshot, "resident:observer", 2, Some(&neg_delta)),
            AutoActivateDecision::AutoReject { .. }
        ));

        // A passing, non-regressive eval → activate.
        let passing = PromptEval {
            prompt_name: "resident:observer".into(),
            candidate_version: 2,
            baseline_version: 1,
            score_delta: 0.42,
            passed: true,
        };
        let d_ok = try_auto_activate(&snapshot, "resident:observer", 2, Some(&passing));
        assert_eq!(d_ok, AutoActivateDecision::Activate);
        assert!(d_ok.is_activate());

        // A mismatched eval (wrong version) does NOT activate.
        let mismatched = PromptEval {
            candidate_version: 99,
            ..passing.clone()
        };
        assert!(matches!(
            try_auto_activate(&snapshot, "resident:observer", 2, Some(&mismatched)),
            AutoActivateDecision::StayProposed { .. }
        ));
    }

    #[test]
    fn mint_plan_proposed_only_when_not_activating() {
        // SI-10 path: no passing eval → the orchestrator mints proposed-only. The
        // plan inserts the row INACTIVE and touches no prior active row.
        let snapshot = vec![rec("resident:observer", 1, "v1", false, true)];
        let plan = mint_plan(&snapshot, "resident:observer", 2, "mode", "v2", false).unwrap();
        assert!(!plan.activate_now);
        assert!(plan.deactivate_versions.is_empty(), "proposed-only touches nothing");
    }

    #[test]
    fn mint_plan_rejects_non_monotonic_and_existing_versions() {
        // A version gap (v1, v3) lets us hit both branches distinctly.
        let snapshot = vec![
            rec("resident:observer", 1, "v1", false, false),
            rec("resident:observer", 3, "v3", false, true),
        ];
        // Re-mint an existing version → VersionExists.
        assert!(matches!(
            mint_plan(&snapshot, "resident:observer", 3, "mode", "x", true),
            Err(MintError::VersionExists { .. })
        ));
        // A NEW version that is still ≤ current max (v2 fills the gap below max v3)
        // → NonMonotonic (mint must move forward, never sideways/backward).
        assert!(matches!(
            mint_plan(&snapshot, "resident:observer", 2, "mode", "x", true),
            Err(MintError::NonMonotonic { current_max: 3, .. })
        ));
        // Monotonic v4 is fine.
        assert!(mint_plan(&snapshot, "resident:observer", 4, "mode", "x", true).is_ok());
    }

    // ---- checksum drift ----------------------------------------------------

    #[test]
    fn checksum_drift_is_flagged_not_overwritten() {
        let snapshot = vec![rec("resident:observer", 2, "edited by hand", false, true)];
        // The manifest said v2's body checksum was the checksum of the ORIGINAL body.
        let mut stored = BTreeMap::new();
        stored.insert(
            ("resident:observer".to_string(), 2),
            checksum_body("original generated body"),
        );
        let drift = detect_drift(&snapshot, &stored);
        assert_eq!(drift.len(), 1, "hand-edit detected as drift");
        assert_eq!(drift[0].name, "resident:observer");
        assert_eq!(drift[0].version, 2);
        assert_eq!(drift[0].actual_checksum, checksum_body("edited by hand"));
        assert_ne!(drift[0].stored_checksum, drift[0].actual_checksum);

        // No drift when the stored checksum matches the current body.
        let mut clean = BTreeMap::new();
        clean.insert(
            ("resident:observer".to_string(), 2),
            checksum_body("edited by hand"),
        );
        assert!(detect_drift(&snapshot, &clean).is_empty());
    }
}

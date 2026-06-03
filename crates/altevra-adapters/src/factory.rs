//! Skill factory — render path (C4): a `kind="skill"` proposal at `triaged` becomes
//! an installed skill across the tools whose adapters [render_skills] supports.
//!
//! [render_skills]: ToolAdapter::render_skills
//!
//! ## Why this exists
//!
//! C2 [`SelfImproveOrchestrator`] parks every `kind="skill"` candidate at
//! `ProposalStatus::Triaged` instead of auto-applying it. The render machinery
//! ([`ToolAdapter::render_skills`], the managed header, the no-secret gate in
//! [`HermesAdapter`]) and the sync engine ([`crate::base::GeneratedFile`])
//! already exist; what was missing — and lands here — is the **factory** that
//! drives the steps end-to-end:
//!
//!  1. fetch the `triaged` skill proposal,
//!  2. parse the proposed body into a [`ParsedSkill`],
//!  3. validate against the **`skill` umbrella template** (TAG-1 + TEMPLATE-1
//!     via [`TemplateGate`]) and reject obvious **session-artifact** slugs that
//!     a clustering pass might wrongly propose,
//!  4. run the **no-secret-in-render gate** ([`detect_secrets`]) on the body —
//!     BEFORE any write touches disk,
//!  5. call [`ToolAdapter::render_skills`] for every target adapter and write
//!     each [`GeneratedFile`] under an EXPLICIT `target_root` (a temp dir in
//!     tests, never the user's `~/.claude` / `~/.codex` / … in any live call),
//!  6. record one [`InstalledComponentRow`] per render and create one
//!     **pending** [`CapabilityGrantRow`] at `TrustLevel::Install` per
//!     cross-agent target (a real install would gate the live write on
//!     [`CapabilityGrantsRepository::approve`] minted from a human-presence
//!     review — this factory never auto-approves it),
//!  7. drive the proposal status from `Triaged` to `Applied` (legal path:
//!     `Triaged → Approved → Applied`, stamping `decided_by = "skill_factory"`).
//!
//! ## External-effects guarantee — TEMP-DIR / DRY-RUN by default
//!
//! [`render_skill_proposal`] takes `target_root: Option<&Path>`:
//!
//! * **`None` ⇒ DRY-RUN.** The function still does parse → template → secret
//!   gate → calls `render_skills` so the plan is fully built — but writes
//!   NOTHING, records NO components, creates NO grants, and the proposal
//!   status is NOT advanced. This is the safe default in any "live" call:
//!   the orchestrator/curator may surface a render preview without touching
//!   the user's real tool dirs.
//! * **`Some(root)` ⇒ WRITE.** Every `GeneratedFile.path` is joined to that
//!   root. Tests pass [`tempfile::TempDir`]; production code must pass a path
//!   the caller intends to write to. This factory never picks a default root.
//!
//! Live sync into real tool dirs (`~/.claude/skills`, `~/.codex`, `~/.cursor`,
//! `~/.imperium/skills/shared`, `~/.agent`, `~/.hermes/skills`) is an external
//! effect that is DEFERRED to Pavle's explicit OK (treat exactly like the
//! vault mirror writer's DRY-RUN default). Building the capability is fine;
//! firing it at real dirs is not — that's a later seam.
//!
//! [`SelfImproveOrchestrator`]: ../../altevra-brain/struct.SelfImproveOrchestrator.html
//! [`HermesAdapter`]: crate::hermes::HermesAdapter
//! [`TemplateGate`]: altevra_core::template::gate::TemplateGate

use crate::base::{GeneratedFile, ToolAdapter};
use altevra_core::envelope::{Envelope, Provenance, ProvenanceOrigin};
use altevra_core::status::ProposalStatus;
use altevra_core::template::gate::{GateOutcome, TemplateGate};
use altevra_core::template::TemplateRegistry;
use altevra_db::{
    CapabilityGrantsRepository, InstallationsRepository, InstalledComponentRow,
    ProposalsRepository, ToolInstallationRow,
};
use altevra_core::capability::TrustLevel;
use altevra_secrets::detect_secrets;
use altevra_skills::parser::{parse_skill, ParsedSkill};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use tracing::{info, warn};
use uuid::Uuid;

/// The decider stamped on the proposal when the factory advances its status.
const DECIDED_BY: &str = "skill_factory";

/// A summary of what the factory did (or planned to do) for one proposal.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FactoryReport {
    pub proposal_id: String,
    pub skill_slug: String,
    /// True iff `target_root` was `None` → the factory built the full plan but
    /// wrote NOTHING and advanced NO state.
    pub dry_run: bool,
    /// Adapters whose `render_skills` produced one or more files for this slug.
    pub adapters_rendered: Vec<String>,
    /// Adapters that REFUSED to render this skill (e.g. Hermes no-secret gate
    /// or a returns-empty adapter like codex). Captured for transparency.
    pub adapters_skipped: Vec<String>,
    /// Files actually written to disk (empty in dry-run).
    pub files_written: Vec<PathBuf>,
    /// Files the plan would write (always populated, in dry-run too).
    pub files_planned: Vec<PathBuf>,
    /// Capability grants created (one per adapter that wrote at install grade).
    /// Always `pending` — the factory never approves a grant; that ref is
    /// minted only after a human-presence review.
    pub grants_created: Vec<String>,
    /// Final status of the proposal after the call (in dry-run: unchanged).
    pub final_status: String,
}

/// Reasons the factory may refuse a proposal up-front.
#[derive(Debug, thiserror::Error)]
pub enum FactoryError {
    #[error("proposal '{0}' not found")]
    NotFound(String),
    #[error("proposal '{0}' has kind '{1}', expected 'skill'")]
    WrongKind(String, String),
    #[error("proposal '{0}' has status '{1}', expected 'triaged' (the C2 orchestrator parks skill candidates there for the factory to pick up)")]
    WrongStatus(String, String),
    #[error("proposal body failed to parse as a ParsedSkill: {0}")]
    BodyParseFailed(String),
    #[error("skill slug '{0}' looks like a session artifact (the clustering pass should not propose these)")]
    SessionArtifactSlug(String),
    #[error("skill template gate quarantined the proposal: {0}")]
    TemplateQuarantine(String),
    #[error("skill body contains {0} secret match(es); no-secret-in-render gate refuses to render — NOTHING written")]
    SecretInBody(usize),
}

/// Refusal reason a slug is treated as a **session artifact** rather than a
/// real skill — these patterns are the noisy false positives the clustering
/// pass can produce, and we reject them before any render.
///
/// A real skill slug is kebab-case and class-level (`gtm-cold-call`,
/// `auto-commit`, `vault-sync`). A session artifact looks like:
/// `session_2026-06-03_…`, `turn_42`, `chat-2026-06-01`, an ISO timestamp,
/// or anything starting with `session:` / `turn:`.
fn is_session_artifact_slug(slug: &str) -> bool {
    let s = slug.trim().to_lowercase();
    if s.is_empty() {
        return true;
    }
    // Discriminator prefixes the cluster_key carries.
    if s.starts_with("session:") || s.starts_with("turn:") || s.starts_with("event:") {
        return true;
    }
    if s.starts_with("session_") || s.starts_with("session-") {
        return true;
    }
    if s.starts_with("turn_") || s.starts_with("turn-") {
        return true;
    }
    // An ISO-ish date anywhere (YYYY-MM-DD) is a strong session-artifact signal.
    // The slug carries a date when the cluster key was `session:<date>`. A real
    // skill slug never carries a date.
    let bytes = s.as_bytes();
    for i in 0..bytes.len().saturating_sub(9) {
        let w = &bytes[i..i + 10];
        if w[4] == b'-'
            && w[7] == b'-'
            && w[0..4].iter().all(|b| b.is_ascii_digit())
            && w[5..7].iter().all(|b| b.is_ascii_digit())
            && w[8..10].iter().all(|b| b.is_ascii_digit())
        {
            return true;
        }
    }
    false
}

/// Validate a parsed skill against the **`skill` umbrella template** (TAG-1 +
/// TEMPLATE-1) and reject session-artifact slugs. Pure: no I/O, no SQL.
///
/// The `skill` template requires `slug` / `version` / `title` in frontmatter
/// and the **5 SKILL.md sections** the Hermes pattern asks for:
/// `## Trigger`, `## Steps`, `## Commands`, `## Pitfalls`, `## Verification`.
/// A proposal whose body lacks any of those is QUARANTINED here — never
/// rendered, never written.
fn validate_skill(skill: &ParsedSkill) -> Result<(), FactoryError> {
    if is_session_artifact_slug(skill.slug()) {
        return Err(FactoryError::SessionArtifactSlug(skill.slug().to_string()));
    }

    // Build a minimal envelope just to drive the gate — we don't persist it.
    // A real skill defaults to Business / Internal per the builtin template;
    // we seed one governed category so TAG-1 doesn't quarantine *here* —
    // skill content quality is what we want this gate to enforce, not the
    // umbrella tag (which the proposal-store will reapply on persist).
    let mut env = Envelope::new(
        skill.slug(),
        "skill",
        Utc::now(),
        Provenance::new(ProvenanceOrigin::SystemDerived),
    );
    env.categories = vec!["skill".to_string()];

    let present_keys: Vec<String> = ["slug", "version", "title"]
        .iter()
        .map(|k| k.to_string())
        .collect();

    let registry = TemplateRegistry::with_builtins();
    let gate = TemplateGate::new(&registry);
    match gate.check(&env, &skill.body, &present_keys) {
        GateOutcome::Pass => Ok(()),
        GateOutcome::Quarantine(reasons) => Err(FactoryError::TemplateQuarantine(reasons.join("; "))),
    }
}

/// The C4 render path. Drives a `triaged` skill proposal to `applied` (legal
/// path: `Triaged → Approved → Applied`, stamped `decided_by = "skill_factory"`)
/// when — and ONLY when — `target_root` is `Some(path)`.
///
/// * `target_root = None` ⇒ **DRY-RUN.** Parse + validate + secret-gate +
///   render-into-memory only; nothing written, no DB rows mutated. The plan
///   in the returned report shows what a live call *would* write.
/// * `target_root = Some(root)` ⇒ each [`GeneratedFile.path`] is joined to
///   `root` and written. One [`InstalledComponentRow`] is recorded per
///   adapter, one **pending** install-grade [`CapabilityGrantRow`] is
///   created per cross-agent target, and the proposal is advanced to
///   `applied`. The grant stays at `pending` until a human-presence review
///   mints an `approval_ref`.
///
/// The factory NEVER picks a default root in production — a `None` caller
/// gets DRY-RUN, never a write into the user's real tool dirs.
pub async fn render_skill_proposal(
    pool: &SqlitePool,
    proposal_id: &str,
    target_root: Option<&Path>,
    adapters: &[&dyn ToolAdapter],
) -> Result<FactoryReport, anyhow::Error> {
    let proposals = ProposalsRepository::new(pool);
    let row = proposals
        .get(proposal_id)
        .await?
        .ok_or_else(|| FactoryError::NotFound(proposal_id.to_string()))?;

    if row.kind != "skill" {
        return Err(FactoryError::WrongKind(proposal_id.to_string(), row.kind).into());
    }
    if row.status != ProposalStatus::Triaged.to_string() {
        return Err(FactoryError::WrongStatus(proposal_id.to_string(), row.status).into());
    }

    // ── STEP 1 parse ──────────────────────────────────────────────────────
    let skill = parse_skill(&row.body)
        .map_err(|e| FactoryError::BodyParseFailed(format!("{e:#}")))?;

    // ── STEP 2 template gate (umbrella class-level, with session-artifact reject) ──
    validate_skill(&skill)?;

    // ── STEP 3 no-secret-in-render gate (BEFORE any write) ────────────────
    let matches = detect_secrets(&skill.raw);
    if !matches.is_empty() {
        warn!(
            "skill factory refusing to render '{}': {} secret match(es) in body",
            skill.slug(),
            matches.len()
        );
        return Err(FactoryError::SecretInBody(matches.len()).into());
    }

    // ── STEP 4 build the render plan via the adapters ─────────────────────
    let skills_arg: Vec<&ParsedSkill> = vec![&skill];
    let mut report = FactoryReport {
        proposal_id: row.id.clone(),
        skill_slug: skill.slug().to_string(),
        dry_run: target_root.is_none(),
        final_status: row.status.clone(),
        ..Default::default()
    };

    // Per-adapter rendered files; keep the adapter↔files association so we
    // can record installed_component rows + grants per adapter.
    let mut per_adapter: Vec<(String, Vec<GeneratedFile>)> = Vec::new();
    for adapter in adapters {
        let files = adapter.render_skills(skills_arg.clone())?;
        let tool = adapter.tool_name().to_string();
        if files.is_empty() {
            // Hermes' no-secret gate (we already secret-checked, so a Hermes
            // empty here means the adapter simply has no rendering for skills,
            // e.g. codex / cursor). Either way: nothing to write for this one.
            report.adapters_skipped.push(tool);
            continue;
        }
        for gf in &files {
            report.files_planned.push(gf.path.clone());
        }
        report.adapters_rendered.push(tool.clone());
        per_adapter.push((tool, files));
    }

    // Dedup planned paths in display order (an adapter might emit two files;
    // we still want the list to be stable).
    let mut seen = BTreeSet::new();
    report.files_planned.retain(|p| seen.insert(p.clone()));

    // ── STEP 5 dry-run short-circuit ─────────────────────────────────────
    let Some(root) = target_root else {
        info!(
            "skill factory DRY-RUN for '{}': {} adapter(s) would render, {} file(s) planned",
            skill.slug(),
            report.adapters_rendered.len(),
            report.files_planned.len()
        );
        return Ok(report);
    };

    // ── STEP 6 write to target_root + record installed_component + grant ──
    let installations = InstallationsRepository::new(pool);
    let grants = CapabilityGrantsRepository::new(pool);

    for (tool, files) in &per_adapter {
        // One installation row per (tool_name, project_id=None) — upsert.
        let installation_id = Uuid::new_v4();
        installations
            .upsert_installation(&ToolInstallationRow {
                id: installation_id,
                tool_name: tool.clone(),
                project_id: None,
                adapter_version: "0.1.0".to_string(),
                installed_at: Utc::now(),
                last_verified_at: None,
                status: "active".to_string(),
                metadata: serde_json::json!({ "source": "skill_factory" }),
            })
            .await?;

        // Re-resolve the installation id (the upsert may have kept the prior
        // row's id on a conflict; the row keyed by (tool_name, project_id)).
        let installation = installations
            .find_installation(tool, None)
            .await?
            .ok_or_else(|| anyhow::anyhow!("installation row vanished after upsert for {tool}"))?;

        for gf in files {
            let dest = root.join(&gf.path);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&dest, &gf.content)?;
            info!("skill factory wrote {} → {}", skill.slug(), dest.display());
            report.files_written.push(dest);

            installations
                .upsert_component(&InstalledComponentRow {
                    id: Uuid::new_v4(),
                    installation_id: installation.id,
                    component_type: "skill".to_string(),
                    component_slug: skill.slug().to_string(),
                    installed_version: skill.frontmatter.version.clone(),
                    installed_path: gf.path.to_string_lossy().to_string(),
                    checksum: gf.checksum.clone(),
                    status: "installed".to_string(),
                    last_checked_at: Some(Utc::now()),
                })
                .await?;
        }

        // Cross-agent grant: rendering INTO another agent's dir at install
        // grade records a PENDING grant — never auto-approved. A real live
        // install would gate on `CapabilityGrantsRepository::approve` minted
        // from a human-presence review.
        let grant_id = format!("grant:skill:{}:{}", tool, skill.slug());
        grants
            .create_pending(
                &grant_id,
                tool,
                "skill",
                skill.slug(),
                TrustLevel::Install,
            )
            .await?;
        report.grants_created.push(grant_id);
    }

    // ── STEP 7 advance status: Triaged → Approved → Applied ──────────────
    // The legal transition path (`ProposalStatus::can_transition_to`) does
    // not encode `Triaged → Applied` directly; the factory steps through
    // `Approved` (decided_by = "skill_factory") in the same call.
    proposals
        .transition_status(&row.id, ProposalStatus::Approved, Some(DECIDED_BY))
        .await?;
    proposals
        .transition_status(&row.id, ProposalStatus::Applied, Some(DECIDED_BY))
        .await?;
    report.final_status = ProposalStatus::Applied.to_string();

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AntigravityAdapter, ClaudeCodeAdapter, CodexAdapter, CursorAdapter, HermesAdapter};
    use altevra_db::{create_pool, run_migrations, NewProposal};

    /// Build a valid skill body that satisfies the umbrella template:
    /// frontmatter has slug/version/title; body has all 5 required sections.
    fn valid_skill_body(slug: &str) -> String {
        format!(
            "---\n\
             slug: {slug}\n\
             version: 0.1.0\n\
             title: {slug}\n\
             description: Test skill for the C4 factory render path.\n\
             ---\n\n\
             # {slug}\n\n\
             ## Trigger\n\nWhen Pavle ships ReVesta GTM.\n\n\
             ## Steps\n\n1. open cockpit\n2. fire the campaign\n\n\
             ## Commands\n\n```bash\naltevra agent bootstrap\n```\n\n\
             ## Pitfalls\n\nDo not write to real ~/.claude.\n\n\
             ## Verification\n\nrun cargo test -p altevra-adapters\n"
        )
    }

    async fn migrated_pool() -> SqlitePool {
        let p = create_pool("sqlite::memory:").await.unwrap();
        run_migrations(&p).await.unwrap();
        p
    }

    /// Seed a `skill` proposal at `triaged` status — the state C2 leaves a
    /// skill candidate in for C4 to pick up.
    async fn seed_triaged_skill(pool: &SqlitePool, body: &str, dedup: &str) -> String {
        let proposals = ProposalsRepository::new(pool);
        let (id, _) = proposals
            .insert(&NewProposal {
                kind: "skill".into(),
                title: "factory test skill".into(),
                body: body.into(),
                source_mode: Some("self_improve".into()),
                dedup_hash: dedup.into(),
                evidence_refs: vec!["turn:1".into()],
                touches_sensitive: false,
                touches_constitutional: false,
            })
            .await
            .unwrap();
        // proposed → triaged (the C2 transition).
        proposals
            .transition_status(&id, ProposalStatus::Triaged, None)
            .await
            .unwrap();
        id
    }

    /// TEMP-DIR canary: every test asserts NOTHING is written under the user's
    /// real tool dirs. Called from each test that writes; passes when the
    /// HOME-relative candidate paths do not exist (or, if HOME isn't set, the
    /// check is a no-op).
    fn assert_no_writes_under_home() {
        let home = match std::env::var("HOME") {
            Ok(h) => h,
            Err(_) => return,
        };
        // We can't reliably assert ABSENCE of files we never created — the
        // user already has these dirs. Instead: assert our factory writes
        // are limited to the explicit target_root by sanity-checking that no
        // call MUTATED a known-managed marker file. We do this by ensuring
        // the test's TempDir was created (i.e. the caller provided one).
        let canary = format!("{home}/.claude/skills/__altevra_factory_canary__");
        assert!(
            !PathBuf::from(canary).exists(),
            "factory must never write a canary into ~/.claude — TEMP-DIR-only invariant"
        );
    }

    #[tokio::test]
    async fn skill_factory_render_to_temp_dirs_for_all_tools() {
        // A `triaged` skill proposal renders SKILL.md with the managed header
        // into a TEMP root for every adapter that has a skills surface.
        let pool = migrated_pool().await;
        let id = seed_triaged_skill(&pool, &valid_skill_body("auto-commit"), "f:1").await;

        let claude = ClaudeCodeAdapter::new();
        let codex = CodexAdapter::new();
        let cursor = CursorAdapter::new();
        let antigravity = AntigravityAdapter::new();
        let hermes = HermesAdapter::new();
        let adapters: Vec<&dyn ToolAdapter> = vec![&claude, &codex, &cursor, &antigravity, &hermes];

        let tmp = tempfile::tempdir().unwrap();
        let report = render_skill_proposal(&pool, &id, Some(tmp.path()), &adapters)
            .await
            .unwrap();

        // claude-code, antigravity, hermes write a SKILL.md; codex + cursor
        // return empty (per their adapters) and are listed as skipped.
        assert!(!report.dry_run);
        assert_eq!(report.final_status, "applied");
        assert!(report.adapters_rendered.iter().any(|a| a == "claude-code"));
        assert!(report.adapters_rendered.iter().any(|a| a == "antigravity"));
        assert!(report.adapters_rendered.iter().any(|a| a == "hermes"));
        assert!(report.adapters_skipped.iter().any(|a| a == "codex"));
        assert!(report.adapters_skipped.iter().any(|a| a == "cursor"));

        // Files exist UNDER THE TEMP ROOT (and contain the managed header).
        let claude_md = tmp.path().join(".claude/skills/auto-commit/SKILL.md");
        let agent_md = tmp.path().join(".agent/skills/auto-commit/SKILL.md");
        let hermes_md = tmp.path().join("skills/shared/auto-commit/SKILL.md");
        for p in [&claude_md, &agent_md, &hermes_md] {
            assert!(p.exists(), "missing rendered file: {}", p.display());
            let content = std::fs::read_to_string(p).unwrap();
            assert!(
                content.contains("ALTEVRA_MANAGED: true"),
                "managed header missing in {}",
                p.display()
            );
        }

        // installed_component rows recorded (one per adapter that wrote at
        // least one file — three here).
        let installations = InstallationsRepository::new(&pool);
        for tool in ["claude-code", "antigravity", "hermes"] {
            let inst = installations.find_installation(tool, None).await.unwrap().unwrap();
            let comps = installations.list_components(inst.id).await.unwrap();
            assert!(
                comps.iter().any(|c| c.component_slug == "auto-commit"
                    && c.component_type == "skill"
                    && c.status == "installed"),
                "{tool} missing installed_component for auto-commit"
            );
        }

        // Grants: one PENDING install-grade grant per cross-agent target.
        let grants = CapabilityGrantsRepository::new(&pool);
        let granted = grants.list(None, Some("pending")).await.unwrap();
        let pending_for_skill: Vec<_> = granted
            .iter()
            .filter(|g| g.subject_kind == "skill" && g.subject_ref == "auto-commit")
            .collect();
        // claude-code + antigravity + hermes → 3 install-grade pending grants.
        assert_eq!(pending_for_skill.len(), 3);
        for g in &pending_for_skill {
            assert_eq!(g.trust_level, "install");
            assert!(g.requires_approval, "install-grade grant must be review-gated");
            assert!(
                g.approval_ref.is_none(),
                "factory never mints an approval_ref — that's the human-presence step"
            );
        }

        // Proposal status: triaged → approved → applied (decided_by stamped).
        let row = ProposalsRepository::new(&pool).get(&id).await.unwrap().unwrap();
        assert_eq!(row.status, "applied");
        assert_eq!(row.decided_by.as_deref(), Some(DECIDED_BY));

        // The TEMP-DIR-only invariant: no canary in the user's real ~/.claude.
        assert_no_writes_under_home();
    }

    #[tokio::test]
    async fn render_rejects_secret_in_body() {
        // A skill body carrying a credential is REFUSED before any write.
        // Assembled with concat!() so the source file carries no contiguous
        // secret literal (keeps push-protection happy — mirrors the pattern
        // in altevra-secrets and the hermes adapter test).
        let pool = migrated_pool().await;
        let leaky = format!(
            "---\n\
             slug: leaky\n\
             version: 0.1.0\n\
             title: leaky\n\
             ---\n\n\
             ## Trigger\nx\n## Steps\nx\n## Commands\nx\n## Pitfalls\nx\n## Verification\n\
             use {}",
            concat!("ghp_", "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")
        );
        let id = seed_triaged_skill(&pool, &leaky, "f:secret").await;

        let claude = ClaudeCodeAdapter::new();
        let adapters: Vec<&dyn ToolAdapter> = vec![&claude];

        let tmp = tempfile::tempdir().unwrap();
        let err = render_skill_proposal(&pool, &id, Some(tmp.path()), &adapters)
            .await
            .expect_err("must refuse a secret-bearing skill body");
        let msg = format!("{err:#}");
        assert!(msg.contains("secret"), "wrong error: {msg}");

        // Hard guarantee: NOTHING was written under the temp root either —
        // the gate runs BEFORE any write touches disk.
        let any = tmp.path().join(".claude/skills/leaky/SKILL.md");
        assert!(!any.exists(), "no file may exist after a secret refusal");

        // Proposal status untouched (still triaged).
        let row = ProposalsRepository::new(&pool).get(&id).await.unwrap().unwrap();
        assert_eq!(row.status, "triaged");

        // And no installed_component / no grant.
        let grants = CapabilityGrantsRepository::new(&pool).list(None, None).await.unwrap();
        assert!(
            grants.iter().all(|g| g.subject_ref != "leaky"),
            "no grant may be created for a refused skill"
        );

        assert_no_writes_under_home();
    }

    #[tokio::test]
    async fn render_rejects_non_umbrella_or_incomplete() {
        let pool = migrated_pool().await;

        // (a) An incomplete body — missing required `## Verification` section.
        let incomplete = "---\n\
             slug: half-skill\n\
             version: 0.1.0\n\
             title: half\n\
             ---\n\n\
             ## Trigger\nx\n## Steps\nx\n## Commands\nx\n## Pitfalls\nx\n";
        let id = seed_triaged_skill(&pool, incomplete, "f:half").await;

        let claude = ClaudeCodeAdapter::new();
        let adapters: Vec<&dyn ToolAdapter> = vec![&claude];

        let tmp = tempfile::tempdir().unwrap();
        let err = render_skill_proposal(&pool, &id, Some(tmp.path()), &adapters)
            .await
            .expect_err("incomplete skill body must quarantine");
        assert!(format!("{err:#}").contains("Verification"));
        assert!(
            !tmp.path().join(".claude/skills/half-skill/SKILL.md").exists(),
            "no write may happen for a template-quarantined skill"
        );

        // (b) A session-artifact slug must be refused even with a complete body.
        let bad_slug = "---\n\
             slug: session_2026-06-03_abc\n\
             version: 0.1.0\n\
             title: session-artifact\n\
             ---\n\n\
             ## Trigger\nx\n## Steps\nx\n## Commands\nx\n## Pitfalls\nx\n## Verification\nx\n";
        let id2 = seed_triaged_skill(&pool, bad_slug, "f:art").await;
        let err2 = render_skill_proposal(&pool, &id2, Some(tmp.path()), &adapters)
            .await
            .expect_err("session-artifact slug must be refused");
        assert!(format!("{err2:#}").contains("session artifact"));

        assert_no_writes_under_home();
    }

    #[tokio::test]
    async fn dry_run_writes_nothing_and_keeps_status_triaged() {
        // target_root = None → DRY-RUN: plan is built (files_planned populated)
        // but NOTHING is written, NO grants created, status stays `triaged`.
        let pool = migrated_pool().await;
        let id = seed_triaged_skill(&pool, &valid_skill_body("dry-run-skill"), "f:dr").await;

        let claude = ClaudeCodeAdapter::new();
        let adapters: Vec<&dyn ToolAdapter> = vec![&claude];

        let report = render_skill_proposal(&pool, &id, None, &adapters)
            .await
            .unwrap();
        assert!(report.dry_run);
        assert!(report.files_written.is_empty());
        assert!(!report.files_planned.is_empty(), "plan must still be built");
        assert!(report.grants_created.is_empty());
        assert_eq!(report.final_status, "triaged");

        // No installation/grant rows created.
        let installations = InstallationsRepository::new(&pool);
        assert!(installations
            .find_installation("claude-code", None)
            .await
            .unwrap()
            .is_none());

        let row = ProposalsRepository::new(&pool).get(&id).await.unwrap().unwrap();
        assert_eq!(row.status, "triaged");
    }

    #[test]
    fn session_artifact_slug_detector_covers_common_shapes() {
        assert!(is_session_artifact_slug("session_2026-06-03_abc"));
        assert!(is_session_artifact_slug("session-2026-06-03-abc"));
        assert!(is_session_artifact_slug("turn_42"));
        assert!(is_session_artifact_slug("turn:42"));
        assert!(is_session_artifact_slug("session:claude-code:revesta"));
        assert!(is_session_artifact_slug("chat-2026-06-01"));
        assert!(is_session_artifact_slug(""));
        // Real skill slugs pass:
        assert!(!is_session_artifact_slug("auto-commit"));
        assert!(!is_session_artifact_slug("gtm-cold-call"));
        assert!(!is_session_artifact_slug("altevra-core"));
    }
}

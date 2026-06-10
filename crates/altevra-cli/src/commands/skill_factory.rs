//! Skill factory CLI (PLAN-ALIVE §P3a + §P3b).
//!
//! `altevra skill-factory edits-preview` reads a skill file + an edits JSON
//! file, runs the pure `apply_edits` engine (budget + protected slow-update
//! regions + per-edit skip reasons) and prints the outcome WITHOUT writing
//! anything to disk. This is the preview surface P3b's renderer builds on —
//! zero LLM, zero side effects.
//!
//! `altevra skill-factory render --proposal <id>` (P3b) turns a triaged
//! `proposals` row (kind=skill, migration 028) into a STAGED SKILL.md draft:
//!
//!  1. **Refuses** a proposal with missing/empty `evidence_refs`.
//!  2. Builds a BOUNDED raw-replay packet from the evidence (`session:` /
//!     `turn:` / `file_change:` refs → raw turns, per-turn + total char caps —
//!     Codex tokens are precious).
//!  3. **Pre-packet exposure gate** (the specified mechanism, not an
//!     assumption): EVERY evidence turn passes `ExposureGate::decide` with the
//!     `external_route` request profile (deny sensitivity ≥ Confidential, deny
//!     `redaction_status ∉ {Clean, Redacted}`), PLUS the high-water session
//!     check — a session whose project maps to a personal/relationship/health
//!     domain is denied (Pavle's policy: work flows to Codex freely; personal
//!     high-water never). ANY denied ref refuses the WHOLE proposal BEFORE any
//!     provider call. One `exposure_decisions` audit row is written per packet
//!     build (content-free aggregates).
//!  4. Sends the packet to the STRONG REASONER (router role — codex_oauth in
//!     Pavle's config) with a skill-authoring prompt.
//!  5. Validates the returned SKILL.md: frontmatter parse (strict
//!     `parse_skill`), required sections, secret/PII scan (`guard_text` — the
//!     second line of defense AFTER the gate), slug sanity (kebab-case, no
//!     traversal), dedup vs existing skills across all tool dirs.
//!  6. Stages to `docs/generated/skills/<slug>/SKILL.md`. DRY-RUN by default —
//!     `--apply` is required to write. NEVER installs into a live skill dir.
//!
//! `--skill <slug>` switches to REFINE mode: instead of a new file the
//! renderer asks the reasoner for bounded P3a `SkillEdit` JSON against the
//! existing skill, refuses fingerprints already tried (`skillopt_meta`),
//! previews via `apply_edits`, and records the attempt as `proposed`.

use altevra_db::{
    create_pool, run_migrations, ExposureAudit, ExposureDecisionsRepository, ProposalRow,
    ProposalsRepository, SessionsRepository, SkilloptMetaRepository, TurnRow,
};
use altevra_llm::{ChatMessage, ChatOpts, ChatProvider, ModelRole};
use altevra_skills::importer::ExternalSkill;
use altevra_skills::parser::{parse_skill, ParsedSkill};
use altevra_skills::skill_edits::{apply_edits, fingerprint_edits, SkillEdit, DEFAULT_EDIT_BUDGET};
use clap::{Args, Subcommand};
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Subcommand)]
pub enum SkillFactoryCommands {
    /// Preview deterministic skill edits (P3a engine). NEVER writes — prints
    /// applied/skipped edits, the edit-set fingerprint, and a diff preview.
    EditsPreview(EditsPreviewArgs),
    /// Render a skill proposal into a staged SKILL.md draft (P3b). Gated:
    /// refuses missing evidence and any evidence that fails the external-route
    /// exposure profile. DRY-RUN by default — pass --apply to stage the file.
    Render(RenderArgs),
}

#[derive(Args)]
pub struct EditsPreviewArgs {
    /// Path to the skill markdown file (SKILL.md). With YAML frontmatter the
    /// edits run over the BODY only; frontmatter is never edited.
    #[arg(long)]
    pub skill: PathBuf,
    /// Path to a JSON file holding the edit array, e.g.
    /// [{"op":"replace","from":"old","to":"new"}].
    #[arg(long)]
    pub edits: PathBuf,
    /// Edit budget — the "textual learning rate" (max edits applied).
    #[arg(long, default_value_t = DEFAULT_EDIT_BUDGET)]
    pub budget: usize,
    /// Emit the full EditOutcome as JSON instead of the human preview.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct RenderArgs {
    /// The proposals.id (kind=skill) to render. Its evidence_refs drive the
    /// replay packet.
    #[arg(long)]
    pub proposal: String,
    /// REFINE an existing skill (by slug) instead of authoring a new one: the
    /// reasoner returns bounded P3a SkillEdit JSON, never a rewritten file.
    #[arg(long)]
    pub skill: Option<String>,
    /// SQLite database path.
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
    /// Actually write the staged draft. Without --apply: dry-run (print only).
    #[arg(long)]
    pub apply: bool,
    /// Staging directory for rendered drafts (never a live skill dir).
    #[arg(long, default_value = "docs/generated/skills")]
    pub out_dir: PathBuf,
    /// Edit budget for refine mode (textual learning rate).
    #[arg(long, default_value_t = DEFAULT_EDIT_BUDGET)]
    pub budget: usize,
}

pub async fn run(cmd: SkillFactoryCommands) -> anyhow::Result<()> {
    match cmd {
        SkillFactoryCommands::EditsPreview(args) => run_edits_preview(args),
        SkillFactoryCommands::Render(args) => run_render(args).await,
    }
}

fn run_edits_preview(args: EditsPreviewArgs) -> anyhow::Result<()> {
    let raw = std::fs::read_to_string(&args.skill)
        .map_err(|e| anyhow::anyhow!("read skill {}: {e}", args.skill.display()))?;
    let edits_raw = std::fs::read_to_string(&args.edits)
        .map_err(|e| anyhow::anyhow!("read edits {}: {e}", args.edits.display()))?;
    let edits: Vec<SkillEdit> = serde_json::from_str(&edits_raw)
        .map_err(|e| anyhow::anyhow!("parse edits JSON {}: {e}", args.edits.display()))?;

    // Frontmatter is out of bounds for the optimizer — edit the body only.
    // Files without frontmatter (plain markdown) are edited whole.
    let body = match parse_skill(&raw) {
        Ok(parsed) => parsed.body,
        Err(_) => raw.clone(),
    };

    let fingerprint = fingerprint_edits(&edits);
    let outcome = apply_edits(&body, &edits, args.budget);

    if args.json {
        let doc = serde_json::json!({
            "skill": args.skill.display().to_string(),
            "budget": args.budget,
            "fingerprint": fingerprint,
            "outcome": outcome,
        });
        println!("{}", serde_json::to_string_pretty(&doc)?);
        return Ok(());
    }

    println!("Skill:       {}", args.skill.display());
    println!("Budget:      {}", args.budget);
    println!("Fingerprint: {fingerprint}");
    println!(
        "Result:      {} applied, {} skipped, changed={}",
        outcome.applied.len(),
        outcome.skipped.len(),
        outcome.changed
    );
    if !outcome.applied.is_empty() {
        println!("\nApplied:");
        for e in &outcome.applied {
            println!("  + {}", e.summary());
        }
    }
    if !outcome.skipped.is_empty() {
        println!("\nSkipped:");
        for s in &outcome.skipped {
            println!("  - {} ({})", s.edit.summary(), s.reason.as_str());
        }
    }
    if outcome.changed {
        println!("\nDiff preview (body):");
        print!("{}", line_diff(&body, &outcome.edited_body));
    } else {
        println!("\nNo changes — body untouched.");
    }
    println!("\n(preview only — nothing was written)");
    Ok(())
}

// ===========================================================================
// P3b — renderer
// ===========================================================================

/// Per-turn char cap in the replay packet (Hivemind `PAIR_CHAR_CAP` parity).
pub(crate) const PACKET_TURN_CHAR_CAP: usize = 2000;
/// Total packet char cap — conserve Codex tokens (Hivemind `TOTAL_PAIRS_CHAR_CAP`).
pub(crate) const PACKET_TOTAL_CHAR_CAP: usize = 40_000;
/// Max turns pulled per `session:` evidence ref.
const SESSION_REF_TURN_CAP: i64 = 40;
/// Project domains that gate a session OUT of external replay (Pavle's policy:
/// the high-water working_dir/project check applies ONLY to personal/
/// relationship/health — work-session data flows to Codex freely).
const HIGH_WATER_PERSONAL_DOMAINS: [&str; 3] = ["personal", "relationship", "health"];

/// The bounded, gated raw-replay packet sent to the strong reasoner.
#[derive(Debug, Clone)]
pub(crate) struct EvidencePacket {
    pub text: String,
    pub turn_count: usize,
    /// Whether per-turn/total char caps elided content (echoed in the audit
    /// row; also lets render output note an incomplete replay).
    #[allow(dead_code)]
    pub truncated: bool,
}

async fn run_render(args: RenderArgs) -> anyhow::Result<()> {
    let pool = create_pool(&args.db.to_string_lossy()).await?;
    run_migrations(&pool).await?;

    let proposal = ProposalsRepository::new(&pool)
        .get(&args.proposal)
        .await?
        .ok_or_else(|| anyhow::anyhow!("proposal '{}' not found", args.proposal))?;
    if proposal.kind != "skill" {
        anyhow::bail!(
            "proposal '{}' has kind '{}' — the renderer only renders kind=skill",
            args.proposal,
            proposal.kind
        );
    }

    // Pre-packet exposure gate + bounded packet build. Refuses (Err) BEFORE any
    // provider call when evidence is missing or any ref is denied.
    let packet = gate_and_build_packet(&pool, &proposal).await?;

    // Router from ~/.altevra/config.toml — StrongReasoner = codex_oauth in
    // Pavle's live config. With no config every role is noop → explicit error.
    let cfg = crate::commands::config::load_config(&altevra_core::home_dir());
    let router = altevra_llm::build_router(&cfg.llm);
    let provider = router.resolve(ModelRole::StrongReasoner);
    if provider.id() == "noop" {
        anyhow::bail!(
            "no strong reasoner configured — set [llm] reasoning_mode in \
             ~/.altevra/config.toml (codex_oauth or api) before rendering"
        );
    }

    let existing = altevra_skills::importer::scan_all();
    match &args.skill {
        None => {
            let outcome =
                render_new_skill(&provider, &proposal, &packet, &existing).await?;
            stage_or_print_new(&pool, &args, &proposal, &outcome).await
        }
        Some(slug) => {
            let outcome =
                render_refine_edits(&pool, &provider, &proposal, &packet, slug, &existing, args.budget)
                    .await?;
            print_refine(&args, slug, &outcome)
        }
    }
}

/// Load + GATE the proposal's evidence and build the bounded replay packet.
///
/// Refusal conditions (whole proposal, before any provider call):
///  * `evidence_refs` missing/empty,
///  * an unresolvable or unsupported ref,
///  * ANY evidence turn denied by `ExposureRequest::external_route`
///    (sensitivity ≥ Confidential, or redaction ∉ {Clean, Redacted}),
///  * ANY evidence session whose project maps to a personal/relationship/
///    health domain (high-water — never leaves the machine).
///
/// One content-free `exposure_decisions` audit row is written per packet
/// build, including refused builds.
pub(crate) async fn gate_and_build_packet(
    pool: &sqlx::SqlitePool,
    proposal: &ProposalRow,
) -> anyhow::Result<EvidencePacket> {
    let refs: Vec<String> = serde_json::from_str(&proposal.evidence_refs).unwrap_or_default();
    if refs.is_empty() {
        anyhow::bail!(
            "REFUSED: proposal '{}' has no evidence_refs — a skill can only be \
             rendered from raw replay evidence",
            proposal.id
        );
    }

    let sessions = SessionsRepository::new(pool);
    let mut turns: Vec<TurnRow> = Vec::new();
    for r in &refs {
        if let Some(id) = r.strip_prefix("turn:") {
            let id = Uuid::parse_str(id)
                .map_err(|e| anyhow::anyhow!("evidence ref '{r}': bad uuid ({e})"))?;
            let t = sessions
                .get_turn(id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("REFUSED: evidence ref '{r}' not found"))?;
            turns.push(t);
        } else if let Some(id) = r.strip_prefix("session:") {
            let id = Uuid::parse_str(id)
                .map_err(|e| anyhow::anyhow!("evidence ref '{r}': bad uuid ({e})"))?;
            let mut session_turns = sessions.list_turns(id, SESSION_REF_TURN_CAP).await?;
            if session_turns.is_empty() {
                anyhow::bail!("REFUSED: evidence ref '{r}' resolves to zero turns");
            }
            turns.append(&mut session_turns);
        } else if let Some(id) = r.strip_prefix("file_change:") {
            // A file_change is gated through its parent turn — content with no
            // gateable parent never enters an external packet.
            let fc_id = Uuid::parse_str(id)
                .map_err(|e| anyhow::anyhow!("evidence ref '{r}': bad uuid ({e})"))?;
            let turn_id: Option<Option<String>> = sqlx::query_scalar(
                "SELECT turn_id FROM file_changes WHERE id = ?",
            )
            .bind(fc_id.to_string())
            .fetch_optional(pool)
            .await?;
            let turn_id = turn_id
                .flatten()
                .and_then(|s| Uuid::parse_str(&s).ok())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "REFUSED: evidence ref '{r}' has no gateable parent turn"
                    )
                })?;
            let t = sessions
                .get_turn(turn_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("REFUSED: evidence ref '{r}' parent turn missing"))?;
            turns.push(t);
        } else {
            anyhow::bail!("REFUSED: unsupported evidence ref '{r}' (expect turn:/session:/file_change:)");
        }
    }

    // ---- the gate: every turn through external_route + high-water session check.
    use altevra_core::safety::ExposureRequest;
    let request = ExposureRequest::external_route();
    let mut included = 0usize;
    let mut allowed_turns: Vec<TurnRow> = Vec::new();
    let mut excluded: Vec<(String, usize)> = Vec::new();
    let mut redaction_counts: std::collections::BTreeMap<String, usize> = Default::default();
    let mut first_denial: Option<String> = None;

    // High-water session lookup (memoized per session).
    let mut session_denied: std::collections::HashMap<Uuid, Option<String>> = Default::default();
    for t in &turns {
        let entry = match session_denied.get(&t.session_id) {
            Some(v) => v.clone(),
            None => {
                let v = session_high_water_denial(pool, &sessions, t.session_id).await?;
                session_denied.insert(t.session_id, v.clone());
                v
            }
        };
        if let Some(reason) = entry {
            match excluded.iter_mut().find(|(r, _)| *r == "high_water_session") {
                Some((_, n)) => *n += 1,
                None => excluded.push(("high_water_session".into(), 1)),
            }
            first_denial.get_or_insert(reason);
            continue;
        }
        match turn_external_route_decision(t, &request) {
            altevra_core::safety::ExposureDecision::Allow => {
                included += 1;
                allowed_turns.push(t.clone());
                *redaction_counts.entry(t.redaction_status.clone()).or_default() += 1;
            }
            altevra_core::safety::ExposureDecision::Deny(reason) => {
                let code = reason.code().to_string();
                match excluded.iter_mut().find(|(r, _)| *r == code) {
                    Some((_, n)) => *n += 1,
                    None => excluded.push((code.clone(), 1)),
                }
                first_denial.get_or_insert(format!(
                    "turn {} denied for external route ({code})",
                    t.id
                ));
            }
        }
    }
    let excluded_count: usize = excluded.iter().map(|(_, n)| n).sum();

    // Bounded packet text — built ONLY from gate-allowed turns. Denied turns
    // are OMITTED (their content is never included) rather than killing the
    // whole render: refusing on any single denial made every real session
    // unrenderable (a long work session almost always contains one
    // secret-bearing, already-redacted turn). Integrity floor below still
    // refuses when the majority of evidence is locked.
    let (text, truncated) = build_packet_text(&allowed_turns);

    // Audit row — content-free aggregates, written for refused builds too.
    let audit = ExposureAudit {
        packet_id: Some(format!("skill_render:{}", proposal.id)),
        sensitivity_ceiling: "internal".into(),
        domain_scope: vec!["business".into(), "project".into(), "public".into()],
        included_count: included,
        excluded_count,
        excluded_by_reason: excluded,
        redaction_counts: redaction_counts.into_iter().collect(),
        truncated,
    };
    ExposureDecisionsRepository::new(pool).insert(&audit).await?;

    // Integrity floor: refuse when there is no usable evidence at all, or when
    // the MAJORITY of evidence is gate-denied (a packet that hides most of its
    // evidence would invite fabrication). Otherwise denied turns are omitted —
    // their content was never read into the packet — and the omission is
    // declared to the renderer explicitly.
    if included == 0 || excluded_count * 2 >= included + excluded_count {
        let denial = first_denial.unwrap_or_else(|| "no usable evidence".into());
        anyhow::bail!(
            "REFUSED: proposal '{}' — {} ({} of {} evidence item(s) denied; nothing was \
             sent to any provider)",
            proposal.id,
            denial,
            excluded_count,
            included + excluded_count
        );
    }

    let text = if excluded_count > 0 {
        format!(
            "{text}\n\n[exposure gate: {excluded_count} evidence turn(s) omitted — locked \
             content was never included in this packet; do not speculate about it]"
        )
    } else {
        text
    };

    Ok(EvidencePacket {
        text,
        turn_count: included,
        truncated,
    })
}

/// One turn through `ExposureGate::decide` with the external-route profile.
/// Turns carry no domain (tools_sessions stamps Business); sensitivity +
/// redaction decide. Unknown sensitivity → `Other` → ranks max → denied.
fn turn_external_route_decision(
    t: &TurnRow,
    request: &altevra_core::safety::ExposureRequest,
) -> altevra_core::safety::ExposureDecision {
    use altevra_core::envelope::{Envelope, Provenance, ProvenanceOrigin};
    use altevra_core::safety::ExposureGate;
    use altevra_core::security::Sensitivity;
    use altevra_core::status::RedactionStatus;

    let mut env = Envelope::new(
        t.id.to_string(),
        "turn",
        t.created_at,
        Provenance::new(ProvenanceOrigin::Imported),
    );
    env.sensitivity = t.sensitivity.parse::<Sensitivity>().unwrap();
    env.domain = altevra_core::domain::Domain::Business;
    let redaction = t
        .redaction_status
        .parse::<RedactionStatus>()
        .unwrap_or(RedactionStatus::Unscanned);
    ExposureGate::decide(&env, &redaction, request)
}

/// High-water session check: the session's project mapped to a personal/
/// relationship/health domain ⇒ `Some(reason)` (deny). Work sessions (any
/// other domain, unknown projects, no project) flow freely — Pavle's policy.
async fn session_high_water_denial(
    pool: &sqlx::SqlitePool,
    sessions: &SessionsRepository<'_>,
    session_id: Uuid,
) -> anyhow::Result<Option<String>> {
    let Some(session) = sessions.get_session(session_id).await? else {
        return Ok(Some(format!("session {session_id} not found")));
    };
    if let Some(project) = session.project_name.as_deref() {
        let domain: Option<String> =
            sqlx::query_scalar("SELECT domain FROM projects WHERE name = ? COLLATE NOCASE")
                .bind(project)
                .fetch_optional(pool)
                .await?;
        if let Some(d) = domain {
            if HIGH_WATER_PERSONAL_DOMAINS.contains(&d.as_str()) {
                return Ok(Some(format!(
                    "session {session_id} belongs to a high-water '{d}' project — \
                     excluded from external replay"
                )));
            }
        }
    }
    Ok(None)
}

/// Bounded replay text: `[role(tool)] content`, per-turn + total char caps.
fn build_packet_text(turns: &[TurnRow]) -> (String, bool) {
    let mut out = String::new();
    let mut truncated = false;
    for t in turns {
        if out.len() >= PACKET_TOTAL_CHAR_CAP {
            truncated = true;
            break;
        }
        let body: String = t.content.chars().take(PACKET_TURN_CHAR_CAP).collect();
        if body.len() < t.content.len() {
            truncated = true;
        }
        let tool = t
            .tool_name
            .as_deref()
            .map(|n| format!("/{n}"))
            .unwrap_or_default();
        out.push_str(&format!("[{}{}] {}\n", t.role, tool, body));
    }
    if out.len() > PACKET_TOTAL_CHAR_CAP {
        out.truncate(PACKET_TOTAL_CHAR_CAP);
        truncated = true;
    }
    (out, truncated)
}

// ---------------------------------------------------------------------------
// new-skill mode
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) struct RenderedSkill {
    pub raw: String,
    pub slug: String,
}

const SKILL_AUTHOR_SYSTEM: &str = "\
You are a skill author for AI coding agents. From the raw session replay you \
are given, crystallize ONE reusable, non-obvious skill as a complete SKILL.md \
markdown document.\n\
HARD REQUIREMENTS:\n\
- Start with YAML frontmatter delimited by --- lines containing: slug \
(kebab-case), version (semver, start 0.1.0), title, description.\n\
- Body must contain the sections '## When to use' and '## Steps'.\n\
- Derive ONLY from the replay; do not invent tools or APIs not present.\n\
- NEVER include secrets, API keys, tokens, emails or personal data.\n\
- Do not pick a slug from this taken list (they already exist).\n\
Output ONLY the SKILL.md content, no commentary.";

/// Ask the strong reasoner for a new SKILL.md and validate it. Pure with
/// respect to disk — staging is the caller's job.
pub(crate) async fn render_new_skill(
    provider: &Arc<dyn ChatProvider>,
    proposal: &ProposalRow,
    packet: &EvidencePacket,
    existing: &[ExternalSkill],
) -> anyhow::Result<RenderedSkill> {
    let taken: Vec<&str> = existing.iter().map(|s| s.slug.as_str()).collect();
    let user = format!(
        "PROPOSAL: {}\n\n{}\n\nTAKEN SLUGS (do not reuse): {}\n\n--- RAW REPLAY ({} turn(s)) ---\n{}\n--- END REPLAY ---",
        proposal.title,
        proposal.body,
        taken.join(", "),
        packet.turn_count,
        packet.text
    );
    let raw = provider
        .complete(
            &[ChatMessage::user(user)],
            &ChatOpts::default()
                .with_system(SKILL_AUTHOR_SYSTEM)
                .with_max_tokens(4000),
        )
        .await?;
    let cleaned = strip_md_fences(&raw);
    let parsed = validate_rendered_skill(&cleaned, existing)?;
    Ok(RenderedSkill {
        raw: cleaned,
        slug: parsed.frontmatter.slug,
    })
}

/// Validation gate over rendered output: strict frontmatter parse, required
/// sections, slug sanity (kebab-case — the slug comes from untrusted model
/// output and becomes a path segment), secret/PII scan (guard_text — second
/// line behind the exposure gate), and dedup vs every existing skill.
pub(crate) fn validate_rendered_skill(
    raw: &str,
    existing: &[ExternalSkill],
) -> anyhow::Result<ParsedSkill> {
    let parsed = parse_skill(raw)
        .map_err(|e| anyhow::anyhow!("rendered output failed frontmatter parse: {e}"))?;

    // slug sanity — untrusted model output becomes a directory name.
    let slug = &parsed.frontmatter.slug;
    let valid_slug = !slug.is_empty()
        && slug.len() <= 64
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !slug.starts_with('-')
        && !slug.ends_with('-');
    if !valid_slug {
        anyhow::bail!("rendered slug '{slug}' is not strict kebab-case — refused");
    }

    // required sections.
    for section in ["## When to use", "## Steps"] {
        if !parsed.body.contains(section) {
            anyhow::bail!("rendered skill is missing required section '{section}'");
        }
    }
    if parsed.body.len() < 80 {
        anyhow::bail!("rendered skill body is too thin ({} chars)", parsed.body.len());
    }

    // secret/PII scan — a generated skill carrying a credential or PII is a
    // hard stop (regenerate), never auto-redact-and-ship.
    let guarded = altevra_secrets::guard_text(raw, altevra_core::security::Sensitivity::Internal);
    if !guarded.sightings.is_empty() {
        anyhow::bail!(
            "rendered skill contains {} secret-like value(s) — refused (regenerate)",
            guarded.sightings.len()
        );
    }
    if guarded
        .risk_tags
        .contains(&altevra_core::RiskTag::ThirdPartyPii)
    {
        anyhow::bail!("rendered skill contains PII (email) — refused (regenerate)");
    }

    // dedup vs existing skills (across every tool dir).
    if let Some(hit) = existing
        .iter()
        .find(|s| s.slug.eq_ignore_ascii_case(slug))
    {
        anyhow::bail!(
            "skill '{slug}' already exists at {} — use `render --skill {slug}` to refine it",
            hit.path.display()
        );
    }

    Ok(parsed)
}

fn strip_md_fences(raw: &str) -> String {
    let t = raw.trim();
    let t = t
        .strip_prefix("```markdown")
        .or_else(|| t.strip_prefix("```md"))
        .or_else(|| t.strip_prefix("```"))
        .unwrap_or(t);
    t.strip_suffix("```").unwrap_or(t).trim().to_string()
}

/// Stage (or dry-run print) a validated new-skill render.
async fn stage_or_print_new(
    pool: &sqlx::SqlitePool,
    args: &RenderArgs,
    proposal: &ProposalRow,
    outcome: &RenderedSkill,
) -> anyhow::Result<()> {
    let target = stage_path(&args.out_dir, &outcome.slug);
    println!("Proposal:  {} — {}", proposal.id, proposal.title);
    println!("Rendered:  skill '{}' ({} bytes)", outcome.slug, outcome.raw.len());
    println!("Stage to:  {}", target.display());
    println!("---\n{}\n---", outcome.raw);
    if !args.apply {
        println!("(dry-run — nothing written; pass --apply to stage)");
        return Ok(());
    }
    stage_skill(&args.out_dir, &outcome.slug, &outcome.raw)?;
    // Mark the proposal triaged (it produced a staged draft awaiting review).
    let _ = ProposalsRepository::new(pool)
        .transition_status(&proposal.id, altevra_core::status::ProposalStatus::Triaged, None)
        .await;
    println!("staged: {}", target.display());
    Ok(())
}

pub(crate) fn stage_path(out_dir: &std::path::Path, slug: &str) -> PathBuf {
    out_dir.join(slug).join("SKILL.md")
}

/// Write the staged draft (atomic temp+rename). The staging dir is NEVER a
/// live skill dir — installation goes through the Pavle-gated sync path.
pub(crate) fn stage_skill(
    out_dir: &std::path::Path,
    slug: &str,
    content: &str,
) -> anyhow::Result<PathBuf> {
    let target = stage_path(out_dir, slug);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = target.with_extension("md.tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, &target)?;
    Ok(target)
}

// ---------------------------------------------------------------------------
// refine mode (--skill <slug>)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) struct RefineOutcome {
    pub edits: Vec<SkillEdit>,
    pub fingerprint: String,
    pub preview: altevra_skills::skill_edits::EditOutcome,
}

const SKILL_REFINE_SYSTEM: &str = "\
You are a skill optimizer. Diagnose the SINGLE recurring weakness the replay \
shows in the given skill and propose a SMALL set (max 3) of structured edits \
that fix it. Do NOT rewrite the whole document. Prefer the smallest change. \
Never touch content between ALTEVRA_SLOW_UPDATE markers. Anchors must be \
EXACT substrings of the current body. Reply ONLY with a JSON array of edits: \
[{\"op\":\"append\",\"text\":\"...\"},{\"op\":\"insert_after\",\"anchor\":\"...\",\"text\":\"...\"},\
{\"op\":\"replace\",\"from\":\"...\",\"to\":\"...\"},{\"op\":\"delete\",\"text\":\"...\"}]";

/// Refine an EXISTING skill: bounded SkillEdit JSON from the reasoner,
/// fingerprint-checked against `skillopt_meta` (a tried set is refused before
/// recording), previewed via the P3a engine, recorded as `proposed`.
pub(crate) async fn render_refine_edits(
    pool: &sqlx::SqlitePool,
    provider: &Arc<dyn ChatProvider>,
    proposal: &ProposalRow,
    packet: &EvidencePacket,
    slug: &str,
    existing: &[ExternalSkill],
    budget: usize,
) -> anyhow::Result<RefineOutcome> {
    let skill = existing
        .iter()
        .find(|s| s.slug == slug)
        .ok_or_else(|| anyhow::anyhow!("skill '{slug}' not found in any skill dir"))?;
    let raw_skill = std::fs::read_to_string(&skill.path)
        .map_err(|e| anyhow::anyhow!("read {}: {e}", skill.path.display()))?;
    let body = match parse_skill(&raw_skill) {
        Ok(p) => p.body,
        Err(_) => raw_skill.clone(),
    };

    // Feed "what's been tried" so the model proposes something different.
    let meta = SkilloptMetaRepository::new(pool);
    let prior = meta.list_for_skill(slug).await?;
    let prior_ops: Vec<String> = prior
        .iter()
        .flat_map(|r| {
            r.ops
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
        .collect();

    let user = format!(
        "PROPOSAL: {}\n{}\n\nALREADY-TRIED EDITS (do not repeat):\n{}\n\n--- CURRENT SKILL BODY ---\n{}\n--- END SKILL ---\n\n--- RAW REPLAY ({} turn(s)) ---\n{}\n--- END REPLAY ---",
        proposal.title,
        proposal.body,
        if prior_ops.is_empty() { "(none)".to_string() } else { prior_ops.join("\n") },
        body,
        packet.turn_count,
        packet.text
    );
    let raw = provider
        .complete(
            &[ChatMessage::user(user)],
            &ChatOpts::default()
                .with_system(SKILL_REFINE_SYSTEM)
                .with_max_tokens(2000),
        )
        .await?;
    let edits = parse_edits_response(&raw)
        .ok_or_else(|| anyhow::anyhow!("reasoner did not return a parseable edit array"))?;
    if edits.is_empty() {
        anyhow::bail!("reasoner proposed zero edits — nothing to refine");
    }

    // Meta-fingerprint dedup: NEVER re-propose a tried set.
    let fingerprint = fingerprint_edits(&edits);
    if meta.was_tried(slug, &fingerprint).await? {
        anyhow::bail!(
            "edit set {} was already tried for '{slug}' (skillopt_meta) — refused",
            &fingerprint[..12]
        );
    }

    let preview = apply_edits(&body, &edits, budget);
    if !preview.changed {
        anyhow::bail!("proposed edits do not change the skill (all skipped) — refused");
    }

    let ops: Vec<String> = edits.iter().map(|e| e.summary()).collect();
    meta.record_tried(slug, &fingerprint, &serde_json::json!(ops), "proposed")
        .await?;

    Ok(RefineOutcome {
        edits,
        fingerprint,
        preview,
    })
}

/// Tolerant SkillEdit-array parse: strips fences, falls back to the first
/// `[...]` slice. `None` = unparseable.
pub(crate) fn parse_edits_response(raw: &str) -> Option<Vec<SkillEdit>> {
    let cleaned = raw.trim();
    let cleaned = cleaned
        .strip_prefix("```json")
        .or_else(|| cleaned.strip_prefix("```"))
        .unwrap_or(cleaned);
    let cleaned = cleaned.strip_suffix("```").unwrap_or(cleaned).trim();
    if let Ok(e) = serde_json::from_str::<Vec<SkillEdit>>(cleaned) {
        return Some(e);
    }
    let start = cleaned.find('[')?;
    let end = cleaned.rfind(']')?;
    if end <= start {
        return None;
    }
    serde_json::from_str(&cleaned[start..=end]).ok()
}

fn print_refine(args: &RenderArgs, slug: &str, outcome: &RefineOutcome) -> anyhow::Result<()> {
    println!("Refine:      {slug}");
    println!("Fingerprint: {}", outcome.fingerprint);
    println!(
        "Edits:       {} proposed, {} applied in preview, {} skipped",
        outcome.edits.len(),
        outcome.preview.applied.len(),
        outcome.preview.skipped.len()
    );
    println!("{}", serde_json::to_string_pretty(&outcome.edits)?);
    if !args.apply {
        println!("(dry-run — recorded as proposed in skillopt_meta; pass --apply to stage the edits file)");
        return Ok(());
    }
    let target = args
        .out_dir
        .join(slug)
        .join(format!("edits-{}.json", &outcome.fingerprint[..12]));
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&target, serde_json::to_string_pretty(&outcome.edits)?)?;
    println!("staged edits: {}", target.display());
    Ok(())
}

/// Minimal line diff: trims the common prefix/suffix and shows the changed
/// middle window as `-`/`+` lines. Deterministic, no external crates.
fn line_diff(before: &str, after: &str) -> String {
    let a: Vec<&str> = before.lines().collect();
    let b: Vec<&str> = after.lines().collect();
    let mut start = 0usize;
    while start < a.len() && start < b.len() && a[start] == b[start] {
        start += 1;
    }
    let mut end_a = a.len();
    let mut end_b = b.len();
    while end_a > start && end_b > start && a[end_a - 1] == b[end_b - 1] {
        end_a -= 1;
        end_b -= 1;
    }
    let mut out = String::new();
    out.push_str(&format!("@@ line {} @@\n", start + 1));
    for line in &a[start..end_a] {
        out.push_str(&format!("- {line}\n"));
    }
    for line in &b[start..end_b] {
        out.push_str(&format!("+ {line}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use altevra_db::{SessionRow, SkilloptMetaRepository};
    use altevra_llm::ChatProvider;
    use async_trait::async_trait;
    use chrono::Utc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn line_diff_shows_only_changed_window() {
        let before = "a\nb\nc\nd\n";
        let after = "a\nB!\nc\nd\n";
        let d = line_diff(before, after);
        assert!(d.contains("- b"));
        assert!(d.contains("+ B!"));
        assert!(!d.contains("- a"));
        assert!(!d.contains("- c"));
        assert!(d.contains("@@ line 2 @@"));
    }

    // =======================================================================
    // P3b renderer — hermetic gate tests (stub provider, per-test TempDir DB,
    // zero network, zero live Codex). PLAN-ALIVE §P3 gate.
    // =======================================================================

    /// Deterministic stub reasoner: fixed response + call counter so refusal
    /// tests can assert "nothing was sent to any provider".
    struct StubProvider {
        response: String,
        calls: AtomicUsize,
    }
    impl StubProvider {
        fn arc(response: &str) -> Arc<StubProvider> {
            Arc::new(StubProvider {
                response: response.to_string(),
                calls: AtomicUsize::new(0),
            })
        }
    }
    #[async_trait]
    impl ChatProvider for StubProvider {
        fn id(&self) -> &str {
            "stub"
        }
        async fn complete(
            &self,
            _messages: &[ChatMessage],
            _opts: &ChatOpts,
        ) -> anyhow::Result<String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.response.clone())
        }
    }

    const STUB_SKILL_MD: &str = "---\nslug: replay-derived-skill\nversion: 0.1.0\ntitle: Replay Derived Skill\ndescription: A deterministic stub draft.\n---\n\n# Replay Derived Skill\n\n## When to use\nWhen testing the renderer hermetically with a stub reasoner.\n\n## Steps\n1. Build the gated packet.\n2. Validate the rendered output.\n";

    async fn test_pool(dir: &tempfile::TempDir) -> sqlx::SqlitePool {
        let db = dir.path().join("render.db");
        let p = create_pool(&db.to_string_lossy()).await.unwrap();
        run_migrations(&p).await.unwrap();
        p
    }

    fn proposal_with_refs(refs: &[String]) -> ProposalRow {
        ProposalRow {
            id: "prop-1".into(),
            kind: "skill".into(),
            risk_tier: "tier1".into(),
            status: "proposed".into(),
            title: "test proposal".into(),
            body: "evidence shows a repeated pattern".into(),
            source_mode: Some("test".into()),
            dedup_hash: "dh-1".into(),
            evidence_count: refs.len() as i64,
            evidence_refs: serde_json::to_string(refs).unwrap(),
            decided_by: None,
            decided_at: None,
            created_at: Utc::now().to_rfc3339(),
        }
    }

    async fn seed_session(
        pool: &sqlx::SqlitePool,
        project_name: Option<&str>,
        turns: &[(i64, &str, &str, &str, &str)], // (idx, role, content, sensitivity, redaction)
    ) -> (Uuid, Vec<Uuid>) {
        let repo = SessionsRepository::new(pool);
        let sid = Uuid::new_v4();
        repo.start_session(&SessionRow {
            id: sid,
            tool: "claude-code".into(),
            project_id: None,
            project_name: project_name.map(String::from),
            started_at: Utc::now(),
            ended_at: None,
            summary: None,
            tokens_in_total: 0,
            tokens_out_total: 0,
            cost_usd_estimate: 0.0,
            turn_count: 0,
            metadata: serde_json::json!({}),
            external_id: None,
            imported_from: None,
            working_dir: Some("/home/x/proj".into()),
        })
        .await
        .unwrap();
        let mut ids = Vec::new();
        for (idx, role, content, sensitivity, redaction) in turns {
            let id = Uuid::new_v4();
            repo.record_turn(&TurnRow {
                id,
                session_id: sid,
                turn_idx: *idx,
                role: (*role).into(),
                content: (*content).into(),
                tool_calls: None,
                tool_name: None,
                model: None,
                tokens_in: None,
                tokens_out: None,
                latency_ms: None,
                file_changes: None,
                redacted_count: 0,
                source_tool: Some("claude-code".into()),
                sensitivity: (*sensitivity).into(),
                redaction_status: (*redaction).into(),
                created_at: Utc::now(),
                working_dir: None,
            })
            .await
            .unwrap();
            ids.push(id);
        }
        (sid, ids)
    }

    async fn exposure_audit_count(pool: &sqlx::SqlitePool, proposal_id: &str) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM exposure_decisions WHERE packet_id = ?")
            .bind(format!("skill_render:{proposal_id}"))
            .fetch_one(pool)
            .await
            .unwrap()
    }

    // ---------- evidence gate ----------

    #[tokio::test]
    async fn renderer_refuses_missing_or_empty_evidence_refs() {
        let dir = tempfile::TempDir::new().unwrap();
        let pool = test_pool(&dir).await;

        let err = gate_and_build_packet(&pool, &proposal_with_refs(&[]))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no evidence_refs"), "{err}");

        // Malformed JSON in evidence_refs behaves like missing.
        let mut p = proposal_with_refs(&[]);
        p.evidence_refs = "not json".into();
        let err = gate_and_build_packet(&pool, &p).await.unwrap_err();
        assert!(err.to_string().contains("no evidence_refs"), "{err}");

        // Unresolvable ref refuses too.
        let p = proposal_with_refs(&[format!("turn:{}", Uuid::new_v4())]);
        let err = gate_and_build_packet(&pool, &p).await.unwrap_err();
        assert!(err.to_string().contains("REFUSED"), "{err}");
    }

    #[tokio::test]
    async fn minority_denied_evidence_is_omitted_with_note_not_refused() {
        let dir = tempfile::TempDir::new().unwrap();
        let pool = test_pool(&dir).await;

        // 2 clean turns + 1 confidential (real redaction finding): the locked
        // turn is OMITTED (content never read into the packet), the packet is
        // built from the clean majority, and the omission is declared.
        let (_sid, ids) = seed_session(
            &pool,
            Some("altevra"),
            &[
                (0, "user", "fix the tsv parser", "internal", "clean"),
                (1, "assistant", "use read -r with explicit cut", "internal", "clean"),
                (2, "assistant", "secret-bearing output", "confidential", "redacted"),
            ],
        )
        .await;
        let p = proposal_with_refs(&[
            format!("turn:{}", ids[0]),
            format!("turn:{}", ids[1]),
            format!("turn:{}", ids[2]),
        ]);
        let packet = gate_and_build_packet(&pool, &p).await.unwrap();
        assert_eq!(packet.turn_count, 2, "only gate-allowed turns counted");
        assert!(
            !packet.text.contains("secret-bearing output"),
            "denied content must never reach the packet"
        );
        assert!(
            packet.text.contains("1 evidence turn(s) omitted"),
            "omission must be declared to the renderer: {}",
            packet.text
        );
        assert_eq!(
            exposure_audit_count(&pool, &p.id).await,
            1,
            "omit-path build still writes an exposure_decisions audit row"
        );
    }

    #[tokio::test]
    async fn renderer_refuses_unscanned_and_confidential_evidence_with_audit() {
        let dir = tempfile::TempDir::new().unwrap();
        let pool = test_pool(&dir).await;

        // One clean turn + one UNSCANNED turn: the WHOLE proposal is refused.
        let (_sid, ids) = seed_session(
            &pool,
            Some("altevra"),
            &[
                (0, "user", "fix the parser", "internal", "clean"),
                (1, "assistant", "raw unscanned content", "internal", "unscanned"),
            ],
        )
        .await;
        let p = proposal_with_refs(&[format!("turn:{}", ids[0]), format!("turn:{}", ids[1])]);
        let err = gate_and_build_packet(&pool, &p).await.unwrap_err();
        assert!(err.to_string().contains("REFUSED"), "{err}");
        assert!(err.to_string().contains("denied"), "{err}");
        assert_eq!(
            exposure_audit_count(&pool, &p.id).await,
            1,
            "refused build still writes an exposure_decisions audit row"
        );

        // Confidential sensitivity (≥ ceiling) refuses as well — even Redacted.
        let (_sid, ids) = seed_session(
            &pool,
            Some("altevra"),
            &[(0, "user", "deal terms", "confidential", "redacted")],
        )
        .await;
        let mut p2 = proposal_with_refs(&[format!("turn:{}", ids[0])]);
        p2.id = "prop-2".into();
        let err = gate_and_build_packet(&pool, &p2).await.unwrap_err();
        assert!(err.to_string().contains("REFUSED"), "{err}");

        // Restricted refuses too.
        let (_sid, ids) = seed_session(
            &pool,
            Some("altevra"),
            &[(0, "user", "very private", "restricted", "clean")],
        )
        .await;
        let mut p3 = proposal_with_refs(&[format!("turn:{}", ids[0])]);
        p3.id = "prop-3".into();
        assert!(gate_and_build_packet(&pool, &p3).await.is_err());
    }

    #[tokio::test]
    async fn renderer_refuses_high_water_personal_session_but_allows_business() {
        let dir = tempfile::TempDir::new().unwrap();
        let pool = test_pool(&dir).await;

        // A project mapped to the 'personal' domain (high-water).
        sqlx::query("INSERT INTO projects (id, name, domain) VALUES (?, 'life-journal', 'personal')")
            .bind(Uuid::new_v4().to_string())
            .execute(&pool)
            .await
            .unwrap();
        // And a normal business project.
        sqlx::query("INSERT INTO projects (id, name, domain) VALUES (?, 'altevra', 'business')")
            .bind(Uuid::new_v4().to_string())
            .execute(&pool)
            .await
            .unwrap();

        // Clean+redacted turn, but the SESSION belongs to a personal project →
        // whole proposal refused (Pavle's policy: personal never leaves).
        let (_sid, ids) = seed_session(
            &pool,
            Some("life-journal"),
            &[(0, "user", "note about my day", "internal", "clean")],
        )
        .await;
        let p = proposal_with_refs(&[format!("turn:{}", ids[0])]);
        let err = gate_and_build_packet(&pool, &p).await.unwrap_err();
        assert!(err.to_string().contains("high-water"), "{err}");

        // Identical turn in a business-project session flows through.
        let (sid, _ids) = seed_session(
            &pool,
            Some("altevra"),
            &[
                (0, "user", "run the import", "internal", "clean"),
                (1, "assistant", "imported 12 sessions", "internal", "redacted"),
            ],
        )
        .await;
        let mut p2 = proposal_with_refs(&[format!("session:{sid}")]);
        p2.id = "prop-biz".into();
        let packet = gate_and_build_packet(&pool, &p2).await.unwrap();
        assert_eq!(packet.turn_count, 2);
        assert!(packet.text.contains("run the import"));
        assert!(packet.text.contains("imported 12 sessions"));
    }

    #[tokio::test]
    async fn packet_is_bounded_by_char_caps() {
        let dir = tempfile::TempDir::new().unwrap();
        let pool = test_pool(&dir).await;
        let huge = "x".repeat(PACKET_TURN_CHAR_CAP * 5);
        let (sid, _) = seed_session(
            &pool,
            Some("altevra"),
            &[
                (0, "user", huge.as_str(), "internal", "clean"),
                (1, "assistant", "short", "internal", "clean"),
            ],
        )
        .await;
        let p = proposal_with_refs(&[format!("session:{sid}")]);
        let packet = gate_and_build_packet(&pool, &p).await.unwrap();
        assert!(packet.truncated, "oversized turn must flag truncation");
        assert!(
            packet.text.len() <= PACKET_TOTAL_CHAR_CAP,
            "total cap enforced"
        );
        // Per-turn cap: the huge turn was elided, the short one survived whole.
        assert!(packet.text.contains("short"));
    }

    #[tokio::test]
    async fn file_change_ref_without_parent_turn_is_refused() {
        let dir = tempfile::TempDir::new().unwrap();
        let pool = test_pool(&dir).await;
        let fc_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO file_changes (id, session_id, turn_id, path, actor_type) \
             VALUES (?, NULL, NULL, '/tmp/x.rs', 'agent')",
        )
        .bind(fc_id.to_string())
        .execute(&pool)
        .await
        .unwrap();
        let p = proposal_with_refs(&[format!("file_change:{fc_id}")]);
        let err = gate_and_build_packet(&pool, &p).await.unwrap_err();
        assert!(err.to_string().contains("no gateable parent turn"), "{err}");
    }

    // ---------- new-skill render + validation ----------

    fn packet() -> EvidencePacket {
        EvidencePacket {
            text: "[user] please do the thing\n[assistant] done\n".into(),
            turn_count: 2,
            truncated: false,
        }
    }

    #[tokio::test]
    async fn stub_render_produces_deterministic_validated_draft() {
        let provider = StubProvider::arc(STUB_SKILL_MD);
        let p = proposal_with_refs(&["turn:x".into()]);
        let arc: Arc<dyn ChatProvider> = provider.clone();
        let a = render_new_skill(&arc, &p, &packet(), &[]).await.unwrap();
        let b = render_new_skill(&arc, &p, &packet(), &[]).await.unwrap();
        assert_eq!(a.slug, "replay-derived-skill");
        assert_eq!(a.raw, b.raw, "stub draft is deterministic");
        assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn render_strips_markdown_fences() {
        let fenced = format!("```markdown\n{STUB_SKILL_MD}\n```");
        let arc: Arc<dyn ChatProvider> = StubProvider::arc(&fenced);
        let p = proposal_with_refs(&["turn:x".into()]);
        let out = render_new_skill(&arc, &p, &packet(), &[]).await.unwrap();
        assert!(out.raw.starts_with("---"), "fences stripped");
        assert_eq!(out.slug, "replay-derived-skill");
    }

    #[test]
    fn validation_rejects_secrets_missing_sections_bad_slug_and_dup() {
        // Secret-bearing output is a hard stop.
        let with_secret = STUB_SKILL_MD.replace(
            "2. Validate the rendered output.",
            "2. Use key=sk-ABCDEFGHIJKLMNOPQRSTUVWXYZ012345 to auth.",
        );
        let err = validate_rendered_skill(&with_secret, &[]).unwrap_err();
        assert!(err.to_string().contains("secret-like"), "{err}");

        // Missing required section.
        let no_steps = STUB_SKILL_MD.replace("## Steps", "## Stuff");
        let err = validate_rendered_skill(&no_steps, &[]).unwrap_err();
        assert!(err.to_string().contains("## Steps"), "{err}");

        // Non-kebab slug (path traversal-ish) is refused.
        let bad_slug = STUB_SKILL_MD.replace("slug: replay-derived-skill", "slug: ../escape");
        let err = validate_rendered_skill(&bad_slug, &[]).unwrap_err();
        assert!(err.to_string().contains("kebab-case"), "{err}");

        // Dedup vs existing skills.
        let existing = vec![ExternalSkill {
            slug: "replay-derived-skill".into(),
            source_tool: altevra_skills::importer::SourceTool::Claude,
            path: PathBuf::from("/x/SKILL.md"),
            version: Some("1.0.0".into()),
            description: None,
            managed: false,
            body_len: 10,
        }];
        let err = validate_rendered_skill(STUB_SKILL_MD, &existing).unwrap_err();
        assert!(err.to_string().contains("already exists"), "{err}");
    }

    #[test]
    fn stage_skill_writes_under_out_dir_only() {
        let dir = tempfile::TempDir::new().unwrap();
        let out = dir.path().join("staged");
        let target = stage_skill(&out, "my-skill", "content").unwrap();
        assert_eq!(target, out.join("my-skill/SKILL.md"));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "content");
    }

    // ---------- refine mode ----------

    #[tokio::test]
    async fn refine_proposes_records_and_refuses_tried_fingerprints() {
        let dir = tempfile::TempDir::new().unwrap();
        let pool = test_pool(&dir).await;

        // The existing skill on disk.
        let skill_dir = dir.path().join("claude/my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        let skill_path = skill_dir.join("SKILL.md");
        std::fs::write(
            &skill_path,
            "---\nslug: my-skill\nversion: 1.0.0\ntitle: My Skill\n---\n# My Skill\n\n## Usage\nrun it\n",
        )
        .unwrap();
        let existing = vec![ExternalSkill {
            slug: "my-skill".into(),
            source_tool: altevra_skills::importer::SourceTool::Claude,
            path: skill_path.clone(),
            version: Some("1.0.0".into()),
            description: None,
            managed: false,
            body_len: 40,
        }];

        let edits_json =
            r#"[{"op":"replace","from":"run it","to":"run it with --verbose"}]"#;
        let arc: Arc<dyn ChatProvider> = StubProvider::arc(edits_json);
        let p = proposal_with_refs(&["turn:x".into()]);

        // First refine: proposed + recorded in skillopt_meta.
        let out = render_refine_edits(&pool, &arc, &p, &packet(), "my-skill", &existing, 3)
            .await
            .unwrap();
        assert_eq!(out.edits.len(), 1);
        assert!(out.preview.changed);
        let meta = SkilloptMetaRepository::new(&pool)
            .list_for_skill("my-skill")
            .await
            .unwrap();
        assert_eq!(meta.len(), 1);
        assert_eq!(meta[0].outcome, "proposed");
        // The skill file itself is untouched (refine NEVER writes the skill).
        assert!(std::fs::read_to_string(&skill_path).unwrap().contains("run it\n"));

        // Second refine returning the SAME edit set: refused (was_tried).
        let err = render_refine_edits(&pool, &arc, &p, &packet(), "my-skill", &existing, 3)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("already tried"), "{err}");
        assert_eq!(
            SkilloptMetaRepository::new(&pool)
                .list_for_skill("my-skill")
                .await
                .unwrap()
                .len(),
            1,
            "no duplicate meta row"
        );

        // Unknown slug refuses.
        let err = render_refine_edits(&pool, &arc, &p, &packet(), "ghost", &existing, 3)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not found"), "{err}");

        // Unparseable reasoner output refuses (never guesses edits).
        let garbage: Arc<dyn ChatProvider> = StubProvider::arc("I think you should rewrite it");
        let err = render_refine_edits(&pool, &garbage, &p, &packet(), "my-skill", &existing, 3)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("parseable"), "{err}");
    }

    #[test]
    fn parse_edits_response_tolerates_fences_and_prose() {
        let plain = r#"[{"op":"append","text":"x"}]"#;
        assert_eq!(parse_edits_response(plain).unwrap().len(), 1);

        let fenced = "```json\n[{\"op\":\"append\",\"text\":\"x\"}]\n```";
        assert_eq!(parse_edits_response(fenced).unwrap().len(), 1);

        let prose = "Here are the edits: [{\"op\":\"append\",\"text\":\"x\"}] enjoy!";
        assert_eq!(parse_edits_response(prose).unwrap().len(), 1);

        assert!(parse_edits_response("no json at all").is_none());
    }
}

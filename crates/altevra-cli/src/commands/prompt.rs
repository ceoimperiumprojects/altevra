use altevra_core::presence::require_human_presence;
use altevra_core::prompts::{
    build_for_tool, PromptInput, PromptOutput, PromptSkill, DEFAULT_UPDATES_LIMIT,
};
use altevra_core::updates::{Importance, UpdateFeedItem};
use altevra_skills::registry::SkillRegistry;
use clap::{Args, Subcommand};
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub enum PromptCommands {
    /// Build the layered system prompt for a given tool
    Build(PromptBuildArgs),
    /// Roll a registry prompt back to an old version by minting a derived active
    /// copy of it. Requires human presence (TTY or ALTEVRA_UNLOCK); an agent/
    /// non-interactive caller is refused. Constitutional-locked slugs (safety,
    /// altevra_rules) are refused by the registry (SI-2).
    Rollback(PromptRollbackArgs),
    /// E2 — review queue for Altevra's self-proposed resident-mode prompt
    /// tweaks. `list`/`show` inspect; `approve` applies the carried unified diff
    /// to the managed region of the mode prompt file via the guarded block
    /// writer (human presence required); `reject` records the fingerprint so the
    /// same tweak is never re-proposed.
    #[command(subcommand)]
    Tweaks(TweakCommands),
}

#[derive(Subcommand)]
pub enum TweakCommands {
    /// List open `prompt_tweak` proposals (defaults to status=proposed).
    List(TweakListArgs),
    /// Show one prompt_tweak proposal in full (mode, target file, diff, reason).
    Show(TweakShowArgs),
    /// Approve a prompt_tweak: apply its diff to the managed region of the mode
    /// prompt file (guarded write: backup + manifest + drift-refuse). Requires
    /// human presence — an agent caller is refused.
    Propose(TweakProposeArgs),
    /// Manually propose a prompt tweak from a diff file (the explicit entry path).
    Approve(TweakApproveArgs),
    /// Reject a prompt_tweak with a reason; records the fingerprint so the same
    /// (mode, diff) is never re-proposed.
    Reject(TweakRejectArgs),
}

#[derive(Args)]
pub struct TweakListArgs {
    /// Filter by status (proposed|triaged|approved|applied|rejected). Default: proposed.
    #[arg(long, default_value = "proposed")]
    pub status: String,
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct TweakShowArgs {
    /// Proposal id.
    pub id: String,
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct TweakProposeArgs {
    /// Resident mode being tweaked (e.g. observer, memory_curator).
    #[arg(long)]
    pub mode: String,
    /// Path to the mode prompt file the diff applies to.
    #[arg(long)]
    pub file: PathBuf,
    /// Path to a file containing the UNIFIED DIFF body.
    #[arg(long)]
    pub diff: PathBuf,
    /// Why this tweak is being proposed.
    #[arg(long)]
    pub reason: String,
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct TweakApproveArgs {
    /// Proposal id to approve + apply.
    pub id: String,
    /// Plan only — do not write the file (still requires presence to run).
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
}

#[derive(Args)]
pub struct TweakRejectArgs {
    /// Proposal id to reject.
    pub id: String,
    /// Reason for the rejection (recorded in the reject memory).
    #[arg(long, default_value = "rejected by Pavle")]
    pub reason: String,
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
}

#[derive(Args)]
pub struct PromptRollbackArgs {
    /// Prompt slug to roll back (e.g. `resident:observer`).
    pub name: String,

    /// The old version whose body becomes the new active version.
    #[arg(long = "to")]
    pub to: i64,

    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
}

#[derive(Args)]
pub struct PromptBuildArgs {
    /// Target tool (claude-code, codex, cursor, antigravity, ...)
    #[arg(long, default_value = "claude-code")]
    pub tool: String,

    /// Project name (looked up under `01-projects/<name>/README.md`)
    #[arg(long)]
    pub project: Option<String>,

    /// Optional current task
    #[arg(long)]
    pub task: Option<String>,

    /// Optional current goal
    #[arg(long)]
    pub goal: Option<String>,

    /// Vault root path
    #[arg(long, default_value = ".")]
    pub vault: PathBuf,

    /// Max recent updates to include in the prompt
    #[arg(long, default_value_t = DEFAULT_UPDATES_LIMIT)]
    pub updates_limit: usize,

    /// Brain database — source of the Tool Register layer (§P2 #7).
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,

    /// Output the full PromptOutput as JSON
    #[arg(long)]
    pub json: bool,
}

pub async fn run(cmd: PromptCommands) -> anyhow::Result<()> {
    match cmd {
        PromptCommands::Build(args) => run_build(args).await,
        PromptCommands::Rollback(args) => run_rollback(args).await,
        PromptCommands::Tweaks(cmd) => run_tweaks(cmd).await,
    }
}

// ---------------------------------------------------------------------------
// E2 — prompt tweak review queue.
// ---------------------------------------------------------------------------

async fn run_tweaks(cmd: TweakCommands) -> anyhow::Result<()> {
    match cmd {
        TweakCommands::List(args) => run_tweaks_list(args).await,
        TweakCommands::Show(args) => run_tweaks_show(args).await,
        TweakCommands::Propose(args) => run_tweaks_propose(args).await,
        TweakCommands::Approve(args) => run_tweaks_approve(args).await,
        TweakCommands::Reject(args) => run_tweaks_reject(args).await,
    }
}

async fn run_tweaks_list(args: TweakListArgs) -> anyhow::Result<()> {
    let pool = altevra_db::create_pool(&args.db.to_string_lossy()).await?;
    altevra_db::run_migrations(&pool).await?;
    let rows = altevra_db::ProposalsRepository::new(&pool)
        .list(Some(&args.status), Some(altevra_brain::PROMPT_TWEAK_KIND))
        .await?;

    if args.json {
        let items: Vec<_> = rows
            .iter()
            .map(|r| {
                let body = altevra_brain::parse_tweak_body(r).ok();
                serde_json::json!({
                    "id": r.id,
                    "status": r.status,
                    "title": r.title,
                    "mode": body.as_ref().map(|b| b.mode.clone()),
                    "target_file": body.as_ref().map(|b| b.target_file.clone()),
                    "evidence_count": r.evidence_count,
                    "created_at": r.created_at,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
    } else if rows.is_empty() {
        println!("No prompt tweaks with status '{}'.", args.status);
    } else {
        println!("Prompt tweaks ({}):", args.status);
        for r in &rows {
            let mode = altevra_brain::parse_tweak_body(r)
                .map(|b| b.mode)
                .unwrap_or_else(|_| "?".into());
            println!("  {} | mode={} | x{} | {}", r.id, mode, r.evidence_count, r.created_at);
        }
    }
    Ok(())
}

async fn run_tweaks_show(args: TweakShowArgs) -> anyhow::Result<()> {
    let pool = altevra_db::create_pool(&args.db.to_string_lossy()).await?;
    altevra_db::run_migrations(&pool).await?;
    let row = altevra_db::ProposalsRepository::new(&pool)
        .get(&args.id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("prompt tweak '{}' not found", args.id))?;
    if row.kind != altevra_brain::PROMPT_TWEAK_KIND {
        anyhow::bail!("proposal '{}' is not a prompt_tweak (kind={})", args.id, row.kind);
    }
    let body = altevra_brain::parse_tweak_body(&row)?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "id": row.id,
                "status": row.status,
                "kind": row.kind,
                "mode": body.mode,
                "target_file": body.target_file,
                "reason": body.reason,
                "fingerprint": body.fingerprint,
                "diff": body.diff,
                "created_at": row.created_at,
            }))?
        );
    } else {
        println!("Prompt tweak {} [{}]", row.id, row.status);
        println!("  mode:        {}", body.mode);
        println!("  target_file: {}", body.target_file);
        println!("  reason:      {}", body.reason);
        println!("  fingerprint: {}", body.fingerprint);
        println!("  --- diff ---");
        for line in body.diff.lines() {
            println!("  {line}");
        }
    }
    Ok(())
}

async fn run_tweaks_propose(args: TweakProposeArgs) -> anyhow::Result<()> {
    let diff = std::fs::read_to_string(&args.diff)
        .map_err(|e| anyhow::anyhow!("read diff file {}: {e}", args.diff.display()))?;
    let pool = altevra_db::create_pool(&args.db.to_string_lossy()).await?;
    altevra_db::run_migrations(&pool).await?;

    let outcome =
        altevra_brain::propose_prompt_tweak(&pool, &args.mode, &args.file, &diff, &args.reason)
            .await?;

    let (status, id): (&str, Option<String>) = match &outcome {
        altevra_brain::ProposeOutcome::Proposed(id) => ("proposed", Some(id.clone())),
        altevra_brain::ProposeOutcome::AlreadyProposed(id) => ("already_proposed", Some(id.clone())),
        altevra_brain::ProposeOutcome::RefusedRejected => ("refused_previously_rejected", None),
        altevra_brain::ProposeOutcome::RefusedInvalidDiff(_) => ("refused_invalid_diff", None),
    };

    if args.json {
        let reason = match &outcome {
            altevra_brain::ProposeOutcome::RefusedInvalidDiff(r) => Some(r.clone()),
            _ => None,
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "status": status, "id": id, "detail": reason,
            }))?
        );
    } else {
        match &outcome {
            altevra_brain::ProposeOutcome::Proposed(id) => {
                println!("proposed prompt tweak {id} for mode '{}' (review with: altevra prompt tweaks show {id})", args.mode);
            }
            altevra_brain::ProposeOutcome::AlreadyProposed(id) => {
                println!("already proposed (merged into {id})");
            }
            altevra_brain::ProposeOutcome::RefusedRejected => {
                println!("refused: this exact tweak was previously rejected — not re-proposing");
            }
            altevra_brain::ProposeOutcome::RefusedInvalidDiff(r) => {
                println!("refused: diff does not apply cleanly — {r}");
            }
        }
    }
    Ok(())
}

async fn run_tweaks_approve(args: TweakApproveArgs) -> anyhow::Result<()> {
    // HP gate FIRST — approving a prompt self-modify is a self-modify; refuse a
    // non-interactive/agent caller before any DB work.
    let proof = require_human_presence().map_err(|e| anyhow::anyhow!("{e}"))?;

    let pool = altevra_db::create_pool(&args.db.to_string_lossy()).await?;
    altevra_db::run_migrations(&pool).await?;
    let proposals = altevra_db::ProposalsRepository::new(&pool);
    let row = proposals
        .get(&args.id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("prompt tweak '{}' not found", args.id))?;
    if row.kind != altevra_brain::PROMPT_TWEAK_KIND {
        anyhow::bail!("proposal '{}' is not a prompt_tweak (kind={})", args.id, row.kind);
    }
    let body = altevra_brain::parse_tweak_body(&row)?;

    let backup_root = altevra_core::home_dir().join(".altevra/backups/prompt-tweak");
    let outcome =
        altevra_brain::apply_prompt_tweak(&pool, &body, &backup_root, !args.dry_run).await?;

    match outcome {
        altevra_brain::ApplyTweakOutcome::Applied { new_block_hash } => {
            if !args.dry_run {
                // Record the human decision through the legal transition path:
                // proposed → approved → applied (both stamp decided_by).
                let decider = format!("pavle:{}", proof.method.as_str());
                proposals
                    .transition_status(&args.id, altevra_core::status::ProposalStatus::Approved, Some(&decider))
                    .await?;
                proposals
                    .transition_status(&args.id, altevra_core::status::ProposalStatus::Applied, Some(&decider))
                    .await?;
                println!(
                    "applied prompt tweak {} to mode '{}' (block {})",
                    args.id, body.mode, &new_block_hash[..new_block_hash.len().min(12)]
                );
            } else {
                println!(
                    "dry-run: tweak {} WOULD apply to mode '{}' (no write performed)",
                    args.id, body.mode
                );
            }
        }
        altevra_brain::ApplyTweakOutcome::DriftRefused => {
            println!(
                "refused: the managed prompt block was edited since Altevra wrote it (drift). \
                 A review item was filed; the file was left byte-identical."
            );
        }
        altevra_brain::ApplyTweakOutcome::DiffNoLongerApplies(reason) => {
            println!("refused: diff no longer applies to the current prompt — {reason}");
        }
        altevra_brain::ApplyTweakOutcome::WriterRefused(reason) => {
            println!("refused by block writer: {reason}");
        }
    }
    Ok(())
}

async fn run_tweaks_reject(args: TweakRejectArgs) -> anyhow::Result<()> {
    let pool = altevra_db::create_pool(&args.db.to_string_lossy()).await?;
    altevra_db::run_migrations(&pool).await?;
    let proposals = altevra_db::ProposalsRepository::new(&pool);
    let row = proposals
        .get(&args.id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("prompt tweak '{}' not found", args.id))?;
    if row.kind != altevra_brain::PROMPT_TWEAK_KIND {
        anyhow::bail!("proposal '{}' is not a prompt_tweak (kind={})", args.id, row.kind);
    }
    let body = altevra_brain::parse_tweak_body(&row)?;

    // Record the fingerprint so the same (mode, diff) is never re-proposed.
    altevra_brain::record_rejected(&pool, &body, &args.reason).await?;
    // Transition the proposal to rejected (records the decision).
    proposals
        .transition_status(&args.id, altevra_core::status::ProposalStatus::Rejected, Some("pavle"))
        .await?;

    println!(
        "rejected prompt tweak {} for mode '{}' — fingerprint recorded (won't re-propose)",
        args.id, body.mode
    );
    Ok(())
}

/// `altevra prompt rollback <name> --to <version>` — mint a derived ACTIVE copy of
/// an old version. This is a self-modify of a registry prompt, so it is gated:
///   1. **Human presence (R4):** refuse unless TTY or ALTEVRA_UNLOCK — an agent
///      may never roll a prompt back.
///   2. **SI-2 (constitutional lock):** the registry refuses a locked slug; the
///      error names the Tier-2 path. Presence alone does not unlock it.
///   3. **SI-8 (one active per slug):** the mint runs deactivate-old-then-
///      activate-new in one transaction.
async fn run_rollback(args: PromptRollbackArgs) -> anyhow::Result<()> {
    // HP gate FIRST — refuse a non-interactive/agent caller before touching the DB.
    let proof = require_human_presence().map_err(|e| anyhow::anyhow!("{e}"))?;

    let pool = altevra_db::create_pool(&args.db.to_string_lossy()).await?;
    altevra_db::run_migrations(&pool).await?;
    let repo = altevra_db::PromptsRepository::new(&pool);

    // Find the source version's body.
    let snapshot = repo.snapshot_for(&args.name).await?;
    let source = snapshot
        .iter()
        .find(|r| r.version == args.to)
        .ok_or_else(|| anyhow::anyhow!("prompt '{}' has no version {}", args.name, args.to))?;
    let layer = source.layer.clone();
    let body = source.body.clone();

    // The derived version is one past the current max (monotonic mint). SI-2 is
    // enforced inside `mint` (a locked slug errors out, no SQL runs).
    let next_version = snapshot.iter().map(|r| r.version).max().unwrap_or(0) + 1;
    let plan = repo
        .mint(&args.name, next_version, &layer, &body, true)
        .await?;

    println!(
        "rolled '{}' back to v{} → minted active v{} (by pavle:{})",
        args.name,
        args.to,
        plan.new_version,
        proof.method.as_str()
    );
    Ok(())
}

async fn run_build(args: PromptBuildArgs) -> anyhow::Result<()> {
    let skills = load_skills_for_prompt(&args.vault)?;
    let recent_updates = load_recent_updates_for_prompt(args.updates_limit);
    let project_readme = args
        .project
        .as_deref()
        .and_then(|p| load_project_readme(&args.vault, p));
    // Tool Register layer (§P2 #7): curated tools between skills and the
    // output protocol. Fault-tolerant — a DB error yields an empty layer.
    let (tools, tools_more) = load_tool_register_for_prompt(&args.db).await;

    let input = PromptInput {
        tool_name: args.tool.clone(),
        project: args.project.clone(),
        current_task: args.task.clone(),
        current_goal: args.goal.clone(),
        recent_updates,
        skills,
        tools,
        tools_more,
        project_readme,
        altevra_version: env!("CARGO_PKG_VERSION").to_string(),
    };

    let output: PromptOutput = build_for_tool(input);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("{}", output.system_prompt);
    }

    Ok(())
}

/// Translate `altevra_skills::parser::ParsedSkill` into the prompt-local
/// `PromptSkill` representation (keeps `altevra-core` decoupled).
fn load_skills_for_prompt(vault: &Path) -> anyhow::Result<Vec<PromptSkill>> {
    let mut registry = SkillRegistry::new();
    let skills_dir = vault.join("06-skills");
    if skills_dir.exists() {
        crate::commands::skill::load_skills_from_dir(&mut registry, &skills_dir)?;
    }

    let mut out = Vec::new();
    for entry in registry.list() {
        let f = &entry.skill.frontmatter;
        let mut sk = PromptSkill::new(&f.slug, &f.version, &f.title);
        sk.description = f.description.clone();
        out.push(sk);
    }
    Ok(out)
}

/// Curated tool summaries (manual/seeded first, capped) + the long-tail count
/// for the prompt's Tool Register layer. Fault-tolerant: any DB error → empty.
async fn load_tool_register_for_prompt(
    db: &Path,
) -> (Vec<altevra_core::session_context::ToolSummary>, usize) {
    let run = async {
        let pool = altevra_db::create_pool(&db.to_string_lossy()).await?;
        altevra_db::run_migrations(&pool).await?;
        let rows = altevra_db::ToolRecordsRepository::new(&pool)
            .list(None, None)
            .await?;
        let total = rows.len();
        let tools: Vec<_> = rows
            .iter()
            .filter(|t| t.source == "manual")
            .take(20)
            .map(|t| altevra_core::session_context::ToolSummary {
                name: t.name.clone(),
                kind: t.kind.clone(),
                invocation: t
                    .invocation
                    .get("canonical")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(no invocation recorded)")
                    .to_string(),
            })
            .collect();
        let more = total.saturating_sub(tools.len());
        anyhow::Ok((tools, more))
    };
    run.await.unwrap_or((vec![], 0))
}

fn load_recent_updates_for_prompt(limit: usize) -> Vec<UpdateFeedItem> {
    let path = altevra_core::home_dir().join(".altevra/events/updates.jsonl");
    if !path.exists() {
        return vec![];
    }
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    let mut items: Vec<UpdateFeedItem> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    items.sort_by_key(|i| std::cmp::Reverse(i.created_at));
    // Filter out noise/low if there are many
    if items.len() > limit {
        items.retain(|i| i.importance >= Importance::Medium);
    }
    items.truncate(limit);
    items
}

fn load_project_readme(vault: &Path, project: &str) -> Option<String> {
    let candidates = [
        vault.join("01-projects").join(project).join("README.md"),
        vault.join("01-projects").join(project).join("readme.md"),
    ];
    for p in candidates {
        if p.exists() {
            return std::fs::read_to_string(&p).ok();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn build_runs_for_claude_code_without_vault() {
        let tmp = TempDir::new().unwrap();
        let args = PromptBuildArgs {
            tool: "claude-code".to_string(),
            project: None,
            task: None,
            goal: None,
            vault: tmp.path().to_path_buf(),
            updates_limit: DEFAULT_UPDATES_LIMIT,
            db: tmp.path().join("prompt-test.db"),
            json: true,
        };
        run_build(args).await.unwrap();
    }

    #[tokio::test]
    async fn build_runs_with_project_and_skills() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join("06-skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        std::fs::write(
            skills_dir.join("altevra-core.md"),
            "---\nslug: altevra-core\nversion: 0.6.0\ntitle: Altevra Core\n---\nBody.",
        )
        .unwrap();
        let proj_dir = tmp.path().join("01-projects").join("altevra");
        std::fs::create_dir_all(&proj_dir).unwrap();
        std::fs::write(proj_dir.join("README.md"), "# Altevra\nLocal-first.").unwrap();

        let args = PromptBuildArgs {
            tool: "codex".to_string(),
            project: Some("altevra".to_string()),
            task: Some("Ship v0.2".to_string()),
            goal: None,
            vault: tmp.path().to_path_buf(),
            updates_limit: 3,
            db: tmp.path().join("prompt-test.db"),
            json: true,
        };
        run_build(args).await.unwrap();
    }

    #[test]
    fn load_project_readme_finds_file() {
        let tmp = TempDir::new().unwrap();
        let proj_dir = tmp.path().join("01-projects").join("foo");
        std::fs::create_dir_all(&proj_dir).unwrap();
        std::fs::write(proj_dir.join("README.md"), "hello").unwrap();
        let body = load_project_readme(tmp.path(), "foo");
        assert_eq!(body.as_deref(), Some("hello"));
    }

    /// `prompt rollback` is a self-modify of a registry prompt → it MUST refuse a
    /// non-interactive (agent) caller. Under `cargo test` stdin is not a TTY; with
    /// `ALTEVRA_UNLOCK` cleared the presence gate refuses BEFORE any DB work, so the
    /// command errors and the registry is never touched.
    #[tokio::test]
    async fn rollback_requires_presence() {
        // Ensure no unlock token leaks in from the ambient env (deterministic refuse).
        std::env::remove_var("ALTEVRA_UNLOCK");
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("a.db");
        let args = PromptRollbackArgs {
            name: "resident:observer".to_string(),
            to: 1,
            db: db.clone(),
        };
        let err = run_rollback(args).await.expect_err("non-TTY must be refused");
        assert!(
            err.to_string().contains("requires_human_presence"),
            "expected presence refusal, got: {err}"
        );
        // The gate runs before create_pool → no DB file is created.
        assert!(!db.exists(), "presence gate must refuse before any DB work");
    }
}

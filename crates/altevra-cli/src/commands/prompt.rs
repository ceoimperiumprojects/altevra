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

    /// Output the full PromptOutput as JSON
    #[arg(long)]
    pub json: bool,
}

pub async fn run(cmd: PromptCommands) -> anyhow::Result<()> {
    match cmd {
        PromptCommands::Build(args) => run_build(args).await,
        PromptCommands::Rollback(args) => run_rollback(args).await,
    }
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

    let input = PromptInput {
        tool_name: args.tool.clone(),
        project: args.project.clone(),
        current_task: args.task.clone(),
        current_goal: args.goal.clone(),
        recent_updates,
        skills,
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

fn load_recent_updates_for_prompt(limit: usize) -> Vec<UpdateFeedItem> {
    let path = Path::new(".altevra/events/updates.jsonl");
    if !path.exists() {
        return vec![];
    }
    let content = match std::fs::read_to_string(path) {
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

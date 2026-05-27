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
    }
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
    items.sort_by(|a, b| b.created_at.cmp(&a.created_at));
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
}

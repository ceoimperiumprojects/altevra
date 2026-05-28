//! `altevra resident` — inspect the resident agent prompt files.
//!
//! Phase 1 surface: listing modes + showing prompt content. The actual
//! resident agent runtime (forking an LLM call with a context packet) lands
//! in Phase 4 once `altevra-llm` multi-provider routing (Phase 2) exists.

use clap::{Args, Subcommand};
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub enum ResidentCommands {
    /// List all resident-agent modes
    Modes(ResidentModesArgs),
    /// Print the system prompt for a specific mode
    Prompt(ResidentPromptArgs),
}

#[derive(Args)]
pub struct ResidentModesArgs {
    /// Skills root (defaults to `06-skills/` under cwd)
    #[arg(long, default_value = "06-skills")]
    pub root: PathBuf,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct ResidentPromptArgs {
    /// Mode name (e.g. memory_curator, synthesis, daily_briefing, wiki_curator, core)
    pub mode: String,
    #[arg(long, default_value = "06-skills")]
    pub root: PathBuf,
}

#[derive(serde::Serialize, Debug)]
struct ModeEntry {
    name: String,
    file: String,
    role: String, // "core" or "mode"
}

pub async fn run(cmd: ResidentCommands) -> anyhow::Result<()> {
    match cmd {
        ResidentCommands::Modes(a) => run_modes(a).await,
        ResidentCommands::Prompt(a) => run_prompt(a).await,
    }
}

async fn run_modes(args: ResidentModesArgs) -> anyhow::Result<()> {
    let modes = enumerate_modes(&args.root);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&modes)?);
    } else {
        println!("Resident agent modes ({}):\n", modes.len());
        for m in &modes {
            println!(
                "  [{role}]  {name:20}  → {file}",
                role = m.role,
                name = m.name,
                file = m.file
            );
        }
        println!("\nRun `altevra resident prompt --mode <name>` to see a mode's system prompt.");
    }
    Ok(())
}

async fn run_prompt(args: ResidentPromptArgs) -> anyhow::Result<()> {
    let path = resolve_mode_path(&args.root, &args.mode)
        .ok_or_else(|| anyhow::anyhow!("unknown resident mode: '{}'", args.mode))?;
    let content = std::fs::read_to_string(&path)?;
    print!("{content}");
    Ok(())
}

fn enumerate_modes(root: &Path) -> Vec<ModeEntry> {
    let mut out = Vec::new();
    let core = root.join("resident-agent-core.md");
    if core.exists() {
        out.push(ModeEntry {
            name: "core".into(),
            file: core.display().to_string(),
            role: "core".into(),
        });
    }
    let modes_dir = root.join("resident-agent-modes");
    if modes_dir.exists() {
        for entry in walkdir::WalkDir::new(&modes_dir)
            .max_depth(1)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let p = entry.path();
            if p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("md") {
                let name = p
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("?")
                    .replace('-', "_");
                out.push(ModeEntry {
                    name,
                    file: p.display().to_string(),
                    role: "mode".into(),
                });
            }
        }
    }
    out.sort_by(|a, b| {
        // core first, then modes alphabetical
        if a.role != b.role {
            return a.role.cmp(&b.role);
        }
        a.name.cmp(&b.name)
    });
    out
}

fn resolve_mode_path(root: &Path, mode: &str) -> Option<PathBuf> {
    if mode == "core" {
        let p = root.join("resident-agent-core.md");
        return p.exists().then_some(p);
    }
    // Accept both `memory_curator` and `memory-curator`.
    let kebab = mode.replace('_', "-");
    let candidates = [
        root.join("resident-agent-modes")
            .join(format!("{kebab}.md")),
        root.join("resident-agent-modes").join(format!("{mode}.md")),
    ];
    candidates.into_iter().find(|p| p.exists())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn seed_skills() -> TempDir {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("resident-agent-modes")).unwrap();
        std::fs::write(
            tmp.path().join("resident-agent-core.md"),
            "---\nid: core\n---\n# Core\nBe useful.\n",
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("resident-agent-modes/synthesis.md"),
            "---\nmode: synthesis\n---\n# Mode: Synthesis\n",
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("resident-agent-modes/memory-curator.md"),
            "---\nmode: memory_curator\n---\n# Mode: Memory Curator\n",
        )
        .unwrap();
        tmp
    }

    #[tokio::test]
    async fn modes_lists_core_and_modes() {
        let tmp = seed_skills();
        let modes = enumerate_modes(tmp.path());
        assert_eq!(modes.len(), 3);
        assert_eq!(modes[0].name, "core");
        assert!(modes.iter().any(|m| m.name == "synthesis"));
        assert!(modes.iter().any(|m| m.name == "memory_curator"));
    }

    #[tokio::test]
    async fn prompt_resolves_underscore_and_hyphen() {
        let tmp = seed_skills();
        let p1 = resolve_mode_path(tmp.path(), "memory_curator").unwrap();
        let p2 = resolve_mode_path(tmp.path(), "memory-curator").unwrap();
        assert_eq!(p1, p2);
        assert!(p1.file_name().unwrap() == "memory-curator.md");
    }

    #[tokio::test]
    async fn prompt_unknown_mode_errors() {
        let tmp = seed_skills();
        let err = run_prompt(ResidentPromptArgs {
            mode: "nonexistent".into(),
            root: tmp.path().to_path_buf(),
        })
        .await;
        assert!(err.is_err());
    }
}

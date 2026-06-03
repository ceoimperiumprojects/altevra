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
    /// Dry-run a resident mode from the registry (P0.5; noop until keys added)
    Run(ResidentRunArgs),
}

#[derive(Args)]
pub struct ResidentRunArgs {
    /// Registry mode name (e.g. memory_curator, personal_curator, insight)
    pub mode: String,
    /// Context packet text to feed the mode (defaults to an empty dry-run packet).
    #[arg(long)]
    pub input: Option<String>,
    /// SQLite database path.
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
    /// Repo/config dir to load `[llm]` settings from (for the model router).
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,
    /// Override the configured reasoning mode for this run (delegated|codex_oauth|api).
    #[arg(long)]
    pub reasoning_mode: Option<String>,
    #[arg(long)]
    pub json: bool,
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
        ResidentCommands::Run(a) => run_resident(a).await,
    }
}

/// Dry-run a registry mode through the resident runtime. P0.5: every role
/// resolves to the noop provider (no keys); the run is recorded into brain_jobs
/// as a `resident_run`. Adding API keys flips the same path live.
async fn run_resident(args: ResidentRunArgs) -> anyhow::Result<()> {
    use altevra_brain::ResidentRunner;

    let pool = altevra_db::create_pool(&args.db.to_string_lossy()).await?;
    altevra_db::run_migrations(&pool).await?;
    let repo = altevra_db::ResidentRepository::new(&pool);
    let mode = repo.get_mode(&args.mode).await?.ok_or_else(|| {
        anyhow::anyhow!(
            "unknown resident mode: '{}' (the registry seeds memory_curator, synthesis, \
             wiki_curator, daily_briefing, insight, observer, personal_curator, skill_factory_proposer)",
            args.mode
        )
    })?;

    // Router from config (delegated/codex_oauth/api). With `delegated` (default) every
    // role resolves to noop — identical to the old hardcoded behavior. SI-7 enforced
    // inside build_router + ModelRouter::resolve.
    let mut cfg = crate::commands::config::load_config(&args.repo);
    if let Some(rm) = args.reasoning_mode.as_deref() {
        cfg.llm.reasoning_mode =
            altevra_core::config::ReasoningMode::parse(rm).ok_or_else(|| {
                anyhow::anyhow!("--reasoning-mode must be: delegated|codex_oauth|api")
            })?;
    }
    let router = altevra_llm::build_router(&cfg.llm);
    let runner = ResidentRunner::new(&router);
    let packet_text = args
        .input
        .clone()
        .unwrap_or_else(|| "(empty context packet — dry run)".to_string());
    let report = runner.run_dry(&mode, &packet_text).await;

    let output_json = serde_json::to_string(&report.output)?;
    let run_id = repo
        .record_run(
            &report.mode,
            &report.model_role,
            &report.provider_id,
            report.status,
            report.dry_run,
            &output_json,
            report.proposals_emitted(),
        )
        .await?;

    // B1: persist the run's proposals into the unified `proposals` table
    // (additive — the brain_jobs output_json above is unchanged). SI-14: a
    // schema-invalid run writes ZERO proposal rows. SI-9: core re-derives each
    // row's risk_tier from its kind (any agent-supplied tier is ignored).
    let proposals_repo = altevra_db::ProposalsRepository::new(&pool);
    let proposal_rows = altevra_db::write_resident_proposals(
        &proposals_repo,
        &report.mode,
        report.status,
        &report.output,
    )
    .await?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "run_id": run_id,
                "mode": report.mode,
                "status": report.status.as_str(),
                "model_role": report.model_role,
                "provider": report.provider_id,
                "proposals_emitted": report.proposals_emitted(),
                "proposal_rows_written": proposal_rows,
                "dry_run": report.dry_run,
            }))?
        );
    } else {
        println!(
            "resident run {} [{}]: {} via {} — {} proposal(s){}",
            &run_id.to_string()[..8],
            report.mode,
            report.status.as_str(),
            report.provider_id,
            report.proposals_emitted(),
            if report.dry_run { " (dry-run)" } else { "" }
        );
        if proposal_rows > 0 {
            println!("  {proposal_rows} proposal row(s) written to the proposals table");
        }
        if report.provider_id == "noop" {
            println!("  (noop provider — add API keys to enable a real model)");
        }
    }
    Ok(())
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

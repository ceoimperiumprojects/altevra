use clap::{Args, Subcommand};

use crate::commands::connect::{resolve_adapter, run as run_connect, ConnectArgs};

#[derive(Subcommand)]
pub enum SetupCommands {
    /// Connect a tool (alias of `connect`)
    Connect(ConnectArgs),
    /// Verify an installation
    Verify(SetupVerifyArgs),
    /// Repair an installation by re-rendering managed files
    Repair(SetupRepairArgs),
    /// Show overall setup status for a tool
    Status(SetupStatusArgs),
    /// v0.3.8 Analyze Everything — import all historical sessions and Obsidian
    /// vault content into Altevra in one shot
    AnalyzeEverything(AnalyzeEverythingArgs),
}

#[derive(Args)]
pub struct AnalyzeEverythingArgs {
    /// Show what would be imported without writing anything
    #[arg(long)]
    pub dry_run: bool,
    /// Skip interactive confirmation
    #[arg(long, short = 'y')]
    pub yes: bool,
    /// Disable automatic Gemini Flash session summaries
    #[arg(long)]
    pub no_llm_summary: bool,
    /// Only import from a specific tool (claude-code | codex | cursor | antigravity | hermes)
    #[arg(long)]
    pub only_tool: Option<String>,
    /// Limit number of sessions per tool (for testing / partial runs)
    #[arg(long)]
    pub limit_per_tool: Option<usize>,
    /// Path to the Altevra SQLite database
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: std::path::PathBuf,
    /// Emit JSON report instead of human-readable text
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct SetupVerifyArgs {
    #[arg(long, default_value = "claude-code")]
    pub tool: String,
    #[arg(long, default_value = ".")]
    pub repo: std::path::PathBuf,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct SetupRepairArgs {
    #[arg(long, default_value = "claude-code")]
    pub tool: String,
    #[arg(long)]
    pub project: Option<String>,
    #[arg(long, default_value = ".")]
    pub repo: std::path::PathBuf,
}

#[derive(Args)]
pub struct SetupStatusArgs {
    #[arg(long, default_value = "claude-code")]
    pub tool: String,
    #[arg(long, default_value = ".")]
    pub repo: std::path::PathBuf,
    #[arg(long)]
    pub json: bool,
}

pub async fn run(cmd: SetupCommands) -> anyhow::Result<()> {
    match cmd {
        SetupCommands::Connect(args) => run_connect(args).await,
        SetupCommands::Verify(args) => run_verify(args).await,
        SetupCommands::Repair(args) => run_repair(args).await,
        SetupCommands::Status(args) => run_status(args).await,
        SetupCommands::AnalyzeEverything(args) => run_analyze_everything(args).await,
    }
}

async fn run_analyze_everything(args: AnalyzeEverythingArgs) -> anyhow::Result<()> {
    use crate::commands::analyze::orchestrator::{
        open_pool, print_report, run_analyze, AnalyzeOpts,
    };

    if let Some(parent) = args.db.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let opts = open_pool(&args.db);
    let pool = sqlx::SqlitePool::connect_with(opts).await?;
    altevra_db::run_migrations(&pool).await?;

    let analyze_opts = AnalyzeOpts {
        dry_run: args.dry_run,
        no_llm_summary: args.no_llm_summary,
        limit_per_tool: args.limit_per_tool,
        only_tool: args.only_tool.clone(),
    };

    // Always run discovery first.
    let report = crate::commands::analyze::discovery::discover();
    if !args.json {
        println!("\nDiscovery:");
        println!(
            "  Claude Code JSONL files:   {}",
            report.claude_code_files.len()
        );
        println!(
            "  Codex state.sqlite:        {}",
            if report.codex_state.is_some() {
                "found"
            } else {
                "—"
            }
        );
        println!(
            "  Codex history.jsonl:       {}",
            if report.codex_history.is_some() {
                "found"
            } else {
                "—"
            }
        );
        println!(
            "  Cursor chatSessions:       {}",
            report.cursor_jsonl_files.len()
        );
        println!(
            "  Antigravity history.jsonl: {}",
            if report.antigravity_history.is_some() {
                "found"
            } else {
                "—"
            }
        );
        println!(
            "  Hermes session_*.json:     {}",
            report.hermes_session_files.len()
        );
        println!(
            "  Obsidian vaults:           {}",
            report.obsidian_vaults.len()
        );
        println!();
    }

    if args.dry_run {
        if args.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "dry_run": true,
                    "total_session_files": report.total_session_files(),
                    "obsidian_vaults": report.obsidian_vaults.len(),
                }))?
            );
        } else {
            println!("(dry-run — no changes made)");
        }
        return Ok(());
    }

    if !args.yes {
        use std::io::{self, Write};
        print!("Proceed with import? [Y/n] ");
        io::stdout().flush().ok();
        let mut line = String::new();
        if io::stdin().read_line(&mut line).is_err()
            || matches!(line.trim().to_lowercase().as_str(), "n" | "no")
        {
            println!("aborted");
            return Ok(());
        }
    }

    let (final_report, stats) = run_analyze(&pool, analyze_opts).await?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "report": {
                    "claude_code_files": final_report.claude_code_files.len(),
                    "codex_history_present": final_report.codex_history.is_some(),
                    "cursor_jsonl_files": final_report.cursor_jsonl_files.len(),
                    "antigravity_present": final_report.antigravity_history.is_some(),
                    "hermes_files": final_report.hermes_session_files.len(),
                    "obsidian_vaults": final_report.obsidian_vaults.len(),
                },
                "stats": stats,
            }))?
        );
    } else {
        print_report(&final_report, &stats);
    }
    Ok(())
}

async fn run_verify(args: SetupVerifyArgs) -> anyhow::Result<()> {
    let adapter = resolve_adapter(&args.tool)?;
    let result = adapter.verify(&args.repo)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!("Verify: {}", args.tool);
        println!("  Status: {}", if result.all_ok { "OK" } else { "ISSUES" });
        for issue in &result.issues {
            println!("  ⚠ {issue}");
        }
        for path in &result.drifted_files {
            println!("  drift: {}", path.display());
        }
    }
    Ok(())
}

async fn run_repair(args: SetupRepairArgs) -> anyhow::Result<()> {
    run_connect(ConnectArgs {
        tool: args.tool,
        project: args.project,
        repo: args.repo,
        dry_run: false,
        force: true,
    })
    .await
}

async fn run_status(args: SetupStatusArgs) -> anyhow::Result<()> {
    let adapter = resolve_adapter(&args.tool)?;
    let detection = adapter.detect(&args.repo);
    let verify = adapter.verify(&args.repo).ok();

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "tool": args.tool,
                "detected": detection.detected,
                "notes": detection.notes,
                "all_ok": verify.as_ref().map(|v| v.all_ok).unwrap_or(false),
                "issues": verify.as_ref().map(|v| v.issues.clone()).unwrap_or_default(),
            }))?
        );
    } else {
        println!("Setup: {}", args.tool);
        println!("  Detected: {}", detection.detected);
        if let Some(v) = verify {
            println!("  Verify: {}", if v.all_ok { "OK" } else { "ISSUES" });
            for issue in v.issues {
                println!("  ⚠ {issue}");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn verify_runs() {
        let tmp = TempDir::new().unwrap();
        run_verify(SetupVerifyArgs {
            tool: "claude-code".to_string(),
            repo: tmp.path().to_path_buf(),
            json: true,
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn status_runs() {
        let tmp = TempDir::new().unwrap();
        run_status(SetupStatusArgs {
            tool: "claude-code".to_string(),
            repo: tmp.path().to_path_buf(),
            json: true,
        })
        .await
        .unwrap();
    }
}

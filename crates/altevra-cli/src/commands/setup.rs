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
    }
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

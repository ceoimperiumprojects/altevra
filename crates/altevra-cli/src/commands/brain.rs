//! `altevra brain` — autonomous brain daemon CLI (v0.3.4).

use altevra_brain::{BrainConfig, BrainScheduler};
use altevra_db::{create_pool, run_migrations};
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum BrainCommands {
    /// Start the brain scheduler. Foreground until Ctrl+C; writes PID file.
    Start(BrainStartArgs),
    /// Show recent job runs + aggregates.
    Status(BrainStatusArgs),
    /// Stop a running brain via SIGTERM.
    Stop(BrainStopArgs),
    /// Run a single tick (useful for cron / one-shot).
    Tick(BrainTickArgs),
}

#[derive(Args)]
pub struct BrainStartArgs {
    #[arg(long, default_value = ".")]
    pub vault: PathBuf,
    #[arg(long, default_value = ".altevra/altevra.db")]
    pub db: PathBuf,
    #[arg(long, default_value_t = 23)]
    pub daily_summary_hour: u32,
    #[arg(long, default_value_t = 30)]
    pub tick_interval_secs: u64,
    /// Comma-separated job kinds to disable.
    #[arg(long, default_value = "")]
    pub disabled: String,
    #[arg(long, default_value = ".altevra/brain.pid")]
    pub pid_file: PathBuf,
}

#[derive(Args)]
pub struct BrainStatusArgs {
    #[arg(long, default_value = ".altevra/altevra.db")]
    pub db: PathBuf,
    #[arg(long, default_value = ".altevra/brain.pid")]
    pub pid_file: PathBuf,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct BrainStopArgs {
    #[arg(long, default_value = ".altevra/brain.pid")]
    pub pid_file: PathBuf,
}

#[derive(Args)]
pub struct BrainTickArgs {
    #[arg(long, default_value = ".")]
    pub vault: PathBuf,
    #[arg(long, default_value = ".altevra/altevra.db")]
    pub db: PathBuf,
    #[arg(long, default_value = "daily_summary")]
    pub disabled: String,
    #[arg(long)]
    pub json: bool,
}

pub async fn run(cmd: BrainCommands) -> anyhow::Result<()> {
    match cmd {
        BrainCommands::Start(args) => run_start(args).await,
        BrainCommands::Status(args) => run_status(args).await,
        BrainCommands::Stop(args) => run_stop(args).await,
        BrainCommands::Tick(args) => run_tick(args).await,
    }
}

fn parse_disabled(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

async fn open_pool(path: &std::path::Path) -> anyhow::Result<sqlx::SqlitePool> {
    let pool = create_pool(&path.to_string_lossy()).await?;
    run_migrations(&pool).await?;
    Ok(pool)
}

async fn run_start(args: BrainStartArgs) -> anyhow::Result<()> {
    let pool = open_pool(&args.db).await?;
    let cfg = BrainConfig {
        vault_path: args.vault,
        db_path: args.db,
        daily_summary_hour: args.daily_summary_hour,
        tick_interval_secs: args.tick_interval_secs,
        disabled: parse_disabled(&args.disabled),
    };

    if let Some(parent) = args.pid_file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&args.pid_file, std::process::id().to_string());

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let tx_signal = shutdown_tx.clone();
    let pid_clean = args.pid_file.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = tx_signal.send(true);
        let _ = std::fs::remove_file(&pid_clean);
    });

    println!(
        "Brain started (PID: {}). Tick interval: {}s. Press Ctrl+C to stop.",
        std::process::id(),
        cfg.tick_interval_secs
    );
    let scheduler = BrainScheduler::new(cfg, pool);
    scheduler.run(shutdown_rx).await?;
    let _ = std::fs::remove_file(&args.pid_file);
    Ok(())
}

async fn run_status(args: BrainStatusArgs) -> anyhow::Result<()> {
    let pool = open_pool(&args.db).await?;
    let mut status = BrainScheduler::status(&pool).await?;
    status.running = args.pid_file.exists();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        println!(
            "Brain status: {}",
            if status.running { "running" } else { "stopped" }
        );
        println!("  Jobs done:   {}", status.jobs_done);
        println!("  Jobs failed: {}", status.jobs_failed);
        if !status.last_runs.is_empty() {
            println!("  Last runs:");
            let mut sorted: Vec<(&String, &String)> = status.last_runs.iter().collect();
            sorted.sort_by(|a, b| a.0.cmp(b.0));
            for (kind, when) in sorted {
                println!("    {kind:<22} {when}");
            }
        }
    }
    Ok(())
}

async fn run_stop(args: BrainStopArgs) -> anyhow::Result<()> {
    if !args.pid_file.exists() {
        anyhow::bail!("No PID file at {}", args.pid_file.display());
    }
    let pid: i32 = std::fs::read_to_string(&args.pid_file)?.trim().parse()?;
    #[cfg(unix)]
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }
    println!("Sent SIGTERM to brain (PID {pid}).");
    Ok(())
}

async fn run_tick(args: BrainTickArgs) -> anyhow::Result<()> {
    let pool = open_pool(&args.db).await?;
    let cfg = BrainConfig {
        vault_path: args.vault,
        db_path: args.db,
        disabled: parse_disabled(&args.disabled),
        ..BrainConfig::default()
    };
    let mut scheduler = BrainScheduler::new(cfg, pool);
    let ran = scheduler.tick().await?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({"jobs_ran": ran}))?
        );
    } else {
        println!("Ran {ran} job(s) in this tick.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn tick_runs_against_fresh_db() {
        let tmp = TempDir::new().unwrap();
        run_tick(BrainTickArgs {
            vault: tmp.path().to_path_buf(),
            db: tmp.path().join("altevra.db"),
            disabled: "daily_summary".into(),
            json: true,
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn status_works_on_empty_db() {
        let tmp = TempDir::new().unwrap();
        run_status(BrainStatusArgs {
            db: tmp.path().join("altevra.db"),
            pid_file: tmp.path().join("brain.pid"),
            json: true,
        })
        .await
        .unwrap();
    }
}

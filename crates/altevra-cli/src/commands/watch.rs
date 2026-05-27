//! `altevra watch` — manage the file watcher daemon (v0.3.2).

use altevra_watcher::{WatcherConfig, WatcherDaemon};
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum WatchCommands {
    /// Start the watcher (foreground; runs until SIGINT).
    Start(WatchStartArgs),
    /// Show watcher status (reads PID file).
    Status(WatchStatusArgs),
    /// Stop a running watcher daemon via SIGTERM.
    Stop(WatchStopArgs),
}

#[derive(Args)]
pub struct WatchStartArgs {
    /// Vault path to watch (repeatable).
    #[arg(long)]
    pub vault: Vec<PathBuf>,

    /// Repo path to watch (repeatable).
    #[arg(long)]
    pub repo: Vec<PathBuf>,

    /// Debounce window (ms).
    #[arg(long, default_value_t = 1000)]
    pub debounce_ms: u64,

    /// Also queue code files (.rs, .ts, .py, ...).
    #[arg(long)]
    pub index_code_files: bool,

    /// SQLite path to write pending_indexing rows.
    #[arg(long, default_value = ".altevra/altevra.db")]
    pub db: PathBuf,

    /// JSONL event log path.
    #[arg(long, default_value = ".altevra/events/file_changes.jsonl")]
    pub event_log: PathBuf,

    /// PID file location.
    #[arg(long, default_value = ".altevra/watcher.pid")]
    pub pid_file: PathBuf,
}

#[derive(Args)]
pub struct WatchStatusArgs {
    #[arg(long, default_value = ".altevra/watcher.pid")]
    pub pid_file: PathBuf,
}

#[derive(Args)]
pub struct WatchStopArgs {
    #[arg(long, default_value = ".altevra/watcher.pid")]
    pub pid_file: PathBuf,
}

pub async fn run(cmd: WatchCommands) -> anyhow::Result<()> {
    match cmd {
        WatchCommands::Start(args) => run_start(args).await,
        WatchCommands::Status(args) => run_status(args).await,
        WatchCommands::Stop(args) => run_stop(args).await,
    }
}

async fn run_start(args: WatchStartArgs) -> anyhow::Result<()> {
    let cfg = WatcherConfig {
        vault_paths: if args.vault.is_empty() {
            vec![PathBuf::from(".")]
        } else {
            args.vault
        },
        repo_paths: args.repo,
        debounce_ms: args.debounce_ms,
        index_code_files: args.index_code_files,
        event_log_path: args.event_log,
        db_path: Some(args.db),
        ..WatcherConfig::default()
    };

    // Write PID file
    if let Some(parent) = args.pid_file.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&args.pid_file, std::process::id().to_string())?;

    let daemon = WatcherDaemon::new(cfg);
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    // Handle Ctrl+C
    let tx_signal = shutdown_tx.clone();
    let pid_file_clean = args.pid_file.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = tx_signal.send(true);
        let _ = std::fs::remove_file(&pid_file_clean);
    });

    println!("Watcher started (PID: {})", std::process::id());
    println!("Press Ctrl+C to stop.");
    daemon.run(shutdown_rx).await?;
    let _ = std::fs::remove_file(&args.pid_file);
    Ok(())
}

async fn run_status(args: WatchStatusArgs) -> anyhow::Result<()> {
    if !args.pid_file.exists() {
        println!(
            "Watcher is not running (no PID file at {}).",
            args.pid_file.display()
        );
        return Ok(());
    }
    let pid_str = std::fs::read_to_string(&args.pid_file)?;
    let pid: u32 = pid_str.trim().parse()?;
    let alive = std::path::Path::new(&format!("/proc/{pid}")).exists();
    if alive {
        println!("Watcher running with PID {pid}.");
    } else {
        println!("Stale PID file (process {pid} not running). Cleaning up.");
        let _ = std::fs::remove_file(&args.pid_file);
    }
    Ok(())
}

async fn run_stop(args: WatchStopArgs) -> anyhow::Result<()> {
    if !args.pid_file.exists() {
        anyhow::bail!("No PID file at {}", args.pid_file.display());
    }
    let pid: i32 = std::fs::read_to_string(&args.pid_file)?.trim().parse()?;
    #[cfg(unix)]
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }
    println!("Sent SIGTERM to watcher (PID {pid}).");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn status_reports_when_no_pid_file() {
        let tmp = TempDir::new().unwrap();
        run_status(WatchStatusArgs {
            pid_file: tmp.path().join("missing.pid"),
        })
        .await
        .unwrap();
    }
}

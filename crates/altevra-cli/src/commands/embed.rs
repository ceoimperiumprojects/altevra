//! `altevra embed` — continuous embedder worker (v0.3.3).
//!
//! Modes:
//!   * `seed`   — enqueue all chunks without vectors
//!   * `tick`   — drain ONE batch and exit (useful for cron / testing)
//!   * `run`    — long-running loop (Ctrl+C to stop)
//!   * `status` — show queue stats
//!
//! In production the worker uses `GeminiEmbedder::from_secrets_or_env`. If no
//! Gemini key is configured, falls back to NoOpEmbedder (zero-dim vectors)
//! so the queue still drains and tests still pass without API access.

use altevra_db::{create_pool, run_migrations};
use altevra_memory::{
    AsyncEmbeddingProvider, EmbedderWorker, EmbedderWorkerConfig, GeminiEmbedder, NoOpEmbedder,
};
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum EmbedCommands {
    /// Enqueue all chunks lacking vectors into the embedder queue.
    Seed(EmbedSeedArgs),
    /// Drain ONE batch and exit.
    Tick(EmbedTickArgs),
    /// Long-running worker (until Ctrl+C).
    Run(EmbedRunArgs),
    /// Show queue stats.
    Status(EmbedStatusArgs),
}

#[derive(Args)]
pub struct EmbedSeedArgs {
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
}

#[derive(Args)]
pub struct EmbedTickArgs {
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
    #[arg(long, default_value_t = 100)]
    pub batch_size: usize,
    /// Use NoOpEmbedder (skip Gemini, useful for testing the queue pipeline).
    #[arg(long)]
    pub noop: bool,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct EmbedRunArgs {
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
    #[arg(long, default_value_t = 100)]
    pub batch_size: usize,
    #[arg(long, default_value_t = 1000)]
    pub rate_limit_rpm: u32,
    #[arg(long)]
    pub noop: bool,
    #[arg(long, default_value = ".altevra/embedder.pid")]
    pub pid_file: PathBuf,
}

#[derive(Args)]
pub struct EmbedStatusArgs {
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
    #[arg(long)]
    pub json: bool,
}

pub async fn run(cmd: EmbedCommands) -> anyhow::Result<()> {
    match cmd {
        EmbedCommands::Seed(args) => run_seed(args).await,
        EmbedCommands::Tick(args) => run_tick(args).await,
        EmbedCommands::Run(args) => run_loop(args).await,
        EmbedCommands::Status(args) => run_status(args).await,
    }
}

async fn open_pool(path: &std::path::Path) -> anyhow::Result<sqlx::SqlitePool> {
    let pool = create_pool(&path.to_string_lossy()).await?;
    run_migrations(&pool).await?;
    Ok(pool)
}

async fn run_seed(args: EmbedSeedArgs) -> anyhow::Result<()> {
    let pool = open_pool(&args.db).await?;
    let worker = EmbedderWorker::new(NoOpEmbedder::new(), pool, EmbedderWorkerConfig::default());
    let n = worker.seed_queue().await?;
    println!("Enqueued {n} chunk(s).");
    Ok(())
}

async fn run_tick(args: EmbedTickArgs) -> anyhow::Result<()> {
    let pool = open_pool(&args.db).await?;
    let cfg = EmbedderWorkerConfig {
        batch_size: args.batch_size,
        rate_limit_rpm: 1000,
        ..EmbedderWorkerConfig::default()
    };
    let n = if args.noop {
        let worker = EmbedderWorker::new(NoOpEmbedder::new(), pool.clone(), cfg);
        worker.tick().await?
    } else {
        match GeminiEmbedder::from_secrets_or_env() {
            Ok(emb) => {
                let worker = EmbedderWorker::new(emb, pool.clone(), cfg);
                worker.tick().await?
            }
            Err(e) => {
                eprintln!("Gemini key not configured ({e}); falling back to NoOp embedder.");
                let worker = EmbedderWorker::new(NoOpEmbedder::new(), pool.clone(), cfg);
                worker.tick().await?
            }
        }
    };
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({"processed": n}))?
        );
    } else {
        println!("Processed {n} chunk(s) in this tick.");
    }
    Ok(())
}

async fn run_loop(args: EmbedRunArgs) -> anyhow::Result<()> {
    let pool = open_pool(&args.db).await?;

    if let Some(parent) = args.pid_file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&args.pid_file, std::process::id().to_string());

    let cfg = EmbedderWorkerConfig {
        batch_size: args.batch_size,
        rate_limit_rpm: args.rate_limit_rpm,
        ..EmbedderWorkerConfig::default()
    };

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let tx_signal = shutdown_tx.clone();
    let pid_clean = args.pid_file.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = tx_signal.send(true);
        let _ = std::fs::remove_file(&pid_clean);
    });

    println!(
        "Embedder running (PID: {}). Rate limit: {} RPM. Press Ctrl+C to stop.",
        std::process::id(),
        args.rate_limit_rpm
    );

    if args.noop {
        let worker = EmbedderWorker::new(NoOpEmbedder::new(), pool, cfg);
        worker.run(shutdown_rx).await?;
    } else {
        match GeminiEmbedder::from_secrets_or_env() {
            Ok(emb) => {
                println!("Provider: {} (dim {})", emb.model_name(), emb.dim());
                let worker = EmbedderWorker::new(emb, pool, cfg);
                worker.run(shutdown_rx).await?;
            }
            Err(e) => {
                eprintln!("Gemini key missing ({e}); using NoOp embedder.");
                let worker = EmbedderWorker::new(NoOpEmbedder::new(), pool, cfg);
                worker.run(shutdown_rx).await?;
            }
        }
    }

    let _ = std::fs::remove_file(&args.pid_file);
    Ok(())
}

async fn run_status(args: EmbedStatusArgs) -> anyhow::Result<()> {
    let pool = open_pool(&args.db).await?;
    let worker = EmbedderWorker::new(NoOpEmbedder::new(), pool, EmbedderWorkerConfig::default());
    let s = worker.stats().await?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "pending": s.pending,
                "in_progress": s.in_progress,
                "done": s.done,
                "failed": s.failed,
            }))?
        );
    } else {
        println!("Embedder queue:");
        println!("  pending:     {}", s.pending);
        println!("  in_progress: {}", s.in_progress);
        println!("  done:        {}", s.done);
        println!("  failed:      {}", s.failed);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn status_on_fresh_db_returns_zeros() {
        let tmp = TempDir::new().unwrap();
        run_status(EmbedStatusArgs {
            db: tmp.path().join("altevra.db"),
            json: true,
        })
        .await
        .unwrap();
    }
}

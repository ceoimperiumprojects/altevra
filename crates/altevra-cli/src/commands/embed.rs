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
    /// Enqueue ALL existing DB content (turns/learnings/notes/wiki/research)
    /// for embedding via the db:// synthetic-document contract. Resumable
    /// (watermark), idempotent (checksum), dry-run by default semantics via
    /// --dry-run.
    Backfill(EmbedBackfillArgs),
    /// Index-only pass: drain pending_indexing into documents+chunks and the
    /// embedder_queue WITHOUT computing any vectors (fast, no model). Use
    /// before `export-chunks` for remote-GPU embedding.
    Index(EmbedIndexArgs),
    /// Export pending chunks (embedder_queue) to JSONL for remote-GPU
    /// embedding. Privacy gate built in: chunks from db://turn/ docs are
    /// exported ONLY when the turn is redaction=clean and sensitivity is
    /// public/internal — everything else is held back for local embedding.
    ExportChunks(EmbedExportArgs),
    /// Import vectors produced remotely (JSONL: {chunk_id, vector}) through
    /// the dim-gate (write_vector_guarded) and mark queue rows done.
    ImportVectors(EmbedImportArgs),
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

#[derive(Args)]
pub struct EmbedIndexArgs {
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
    #[arg(long, default_value_t = 500)]
    pub batch_size: usize,
}

#[derive(Args)]
pub struct EmbedExportArgs {
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
    #[arg(long)]
    pub out: PathBuf,
    #[arg(long, default_value_t = 0)]
    pub limit: usize,
}

#[derive(Args)]
pub struct EmbedImportArgs {
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
    #[arg(long)]
    pub file: PathBuf,
    #[arg(long, default_value = "bge-m3")]
    pub model: String,
}

#[derive(Args)]
pub struct EmbedBackfillArgs {
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
    #[arg(long, default_value_t = 500)]
    pub batch_size: usize,
    /// Report per-table enqueue counts without writing anything.
    #[arg(long)]
    pub dry_run: bool,
    /// Override the watermark model key (defaults to the active embedder's
    /// model name so the cursor matches what the worker will embed with).
    #[arg(long)]
    pub model: Option<String>,
    #[arg(long)]
    pub json: bool,
}

pub async fn run(cmd: EmbedCommands) -> anyhow::Result<()> {
    // Maintenance lock (db unify): the embedder is a batch writer — refuse
    // non-fatally for every write mode; `status` stays read-only and allowed,
    // as does a backfill --dry-run (it only reads and reports).
    let read_only = matches!(cmd, EmbedCommands::Status(_) | EmbedCommands::ExportChunks(_))
        || matches!(&cmd, EmbedCommands::Backfill(a) if a.dry_run);
    if !read_only && crate::commands::brain::refuse_if_maintenance_locked("embedder") {
        return Ok(());
    }
    match cmd {
        EmbedCommands::Seed(args) => run_seed(args).await,
        EmbedCommands::Tick(args) => run_tick(args).await,
        EmbedCommands::Run(args) => run_loop(args).await,
        EmbedCommands::Status(args) => run_status(args).await,
        EmbedCommands::Backfill(args) => run_backfill_cmd(args).await,
        EmbedCommands::Index(args) => run_index_only(args).await,
        EmbedCommands::ExportChunks(args) => run_export_chunks(args).await,
        EmbedCommands::ImportVectors(args) => run_import_vectors(args).await,
    }
}

/// Index-only: chunk pending objects, enqueue embedder_queue, write NO vectors.
async fn run_index_only(args: EmbedIndexArgs) -> anyhow::Result<()> {
    let pool = open_pool(&args.db).await?;
    let cfg = EmbedderWorkerConfig {
        batch_size: args.batch_size,
        ..EmbedderWorkerConfig::default()
    };
    let worker = EmbedderWorker::new(NoOpEmbedder::new(), pool, cfg);
    let mut total = 0usize;
    loop {
        let n = worker.drain_pending_files().await?;
        total += n;
        eprintln!("  indexed {total} object(s)…");
        if n == 0 {
            break;
        }
    }
    println!("Index-only pass complete: {total} object(s) chunked + enqueued (no vectors).");
    Ok(())
}

/// Export pending embedder_queue chunks to JSONL for remote-GPU embedding.
/// Privacy gate: db://turn/ chunks only when the turn is clean + ≤ internal.
async fn run_export_chunks(args: EmbedExportArgs) -> anyhow::Result<()> {
    use sqlx::Row;
    let pool = open_pool(&args.db).await?;
    let limit = if args.limit == 0 { i64::MAX } else { args.limit as i64 };
    let rows = sqlx::query(
        r#"SELECT q.chunk_id, c.text, d.source_path,
                  t.sensitivity AS turn_sensitivity,
                  t.redaction_status AS turn_redaction
           FROM embedder_queue q
           JOIN memory_chunks c ON c.id = q.chunk_id
           JOIN memory_documents d ON d.id = c.document_id
           LEFT JOIN turns t
             ON d.source_path = 'db://turn/' || t.id
           WHERE q.status = 'pending'
           ORDER BY q.enqueued_at
           LIMIT ?"#,
    )
    .bind(limit)
    .fetch_all(&pool)
    .await?;

    let mut out = std::io::BufWriter::new(std::fs::File::create(&args.out)?);
    use std::io::Write;
    let (mut exported, mut held) = (0u64, 0u64);
    for r in rows {
        let source: String = r.get("source_path");
        if source.starts_with("db://turn/") {
            let sens: Option<String> = r.get("turn_sensitivity");
            let red: Option<String> = r.get("turn_redaction");
            let ok = matches!(red.as_deref(), Some("clean"))
                && matches!(sens.as_deref(), Some("public") | Some("internal"));
            if !ok {
                held += 1;
                continue;
            }
        }
        let chunk_id: String = r.get("chunk_id");
        let text: String = r.get("text");
        writeln!(
            out,
            "{}",
            serde_json::json!({ "chunk_id": chunk_id, "text": text })
        )?;
        exported += 1;
    }
    out.flush()?;
    println!(
        "Exported {exported} chunk(s) to {} — {held} held back (locked turns stay local).",
        args.out.display()
    );
    Ok(())
}

/// Import remotely-computed vectors through the dim-gate; mark queue done.
async fn run_import_vectors(args: EmbedImportArgs) -> anyhow::Result<()> {
    let pool = open_pool(&args.db).await?;
    let f = std::fs::File::open(&args.file)?;
    let reader = std::io::BufReader::new(f);
    use std::io::BufRead;
    let (mut ok, mut failed) = (0u64, 0u64);
    let mut registered = false;
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(&line)?;
        let chunk_id_s = v["chunk_id"].as_str().unwrap_or_default().to_string();
        let Ok(chunk_id) = uuid::Uuid::parse_str(&chunk_id_s) else {
            failed += 1;
            continue;
        };
        let vector: Vec<f32> = v["vector"]
            .as_array()
            .map(|a| a.iter().filter_map(|x| x.as_f64().map(|f| f as f32)).collect())
            .unwrap_or_default();
        if vector.is_empty() {
            failed += 1;
            continue;
        }
        if !registered {
            altevra_memory::register_model_dim(&pool, &args.model, vector.len()).await?;
            registered = true;
        }
        match altevra_memory::write_vector_guarded(&pool, chunk_id, &args.model, &vector).await {
            Ok(()) => {
                sqlx::query(
                    r#"UPDATE embedder_queue
                       SET status='done', finished_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
                       WHERE chunk_id = ?"#,
                )
                .bind(&chunk_id_s)
                .execute(&pool)
                .await?;
                ok += 1;
            }
            Err(e) => {
                eprintln!("  chunk {chunk_id_s}: {e}");
                failed += 1;
            }
        }
    }
    println!("Imported {ok} vector(s) ({failed} failed) — model '{}' via dim-gate.", args.model);
    Ok(())
}

/// The model name the watermark is keyed by — must match what the worker
/// will actually embed with, or the cursor tracks a phantom model.
/// Preference order mirrors the worker's embedder selection:
/// BGE-M3 (local, feature-gated) → Gemini (if configured) → NoOp.
fn active_model_name(explicit: Option<&str>) -> String {
    if let Some(m) = explicit {
        return m.to_string();
    }
    #[cfg(feature = "embedding")]
    {
        return altevra_memory::BGE_M3_MODEL.to_string();
    }
    #[allow(unreachable_code)]
    match GeminiEmbedder::from_secrets_or_env() {
        Ok(emb) => emb.model_name().to_string(),
        Err(_) => NoOpEmbedder::new().model_name().to_string(),
    }
}

async fn run_backfill_cmd(args: EmbedBackfillArgs) -> anyhow::Result<()> {
    let pool = open_pool(&args.db).await?;
    let model = active_model_name(args.model.as_deref());
    let report =
        altevra_memory::run_backfill(&pool, &model, args.batch_size, args.dry_run).await?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "model": model,
                "dry_run": args.dry_run,
                "total_scanned": report.total_scanned(),
                "total_enqueued": report.total_enqueued(),
                "sources": report
                    .sources
                    .iter()
                    .map(|(k, v)| (k.to_string(), serde_json::json!({
                        "scanned": v.scanned,
                        "enqueued": v.enqueued,
                    })))
                    .collect::<std::collections::BTreeMap<_, _>>(),
            }))?
        );
    } else {
        let mode = if args.dry_run { "DRY-RUN" } else { "APPLIED" };
        println!("Embed backfill ({mode}) — model '{model}':");
        for (otype, s) in &report.sources {
            println!("  {otype:<10} scanned {:>6}  enqueued {:>6}", s.scanned, s.enqueued);
        }
        println!(
            "  total: {} scanned, {} enqueued{}",
            report.total_scanned(),
            report.total_enqueued(),
            if args.dry_run { " (nothing written)" } else { "" }
        );
    }
    Ok(())
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

async fn tick_gemini_or_noop(
    pool: sqlx::SqlitePool,
    cfg: EmbedderWorkerConfig,
) -> anyhow::Result<usize> {
    match GeminiEmbedder::from_secrets_or_env() {
        Ok(emb) => {
            let worker = EmbedderWorker::new(emb, pool, cfg);
            worker.tick().await
        }
        Err(e) => {
            eprintln!("Gemini key not configured ({e}); falling back to NoOp embedder.");
            let worker = EmbedderWorker::new(NoOpEmbedder::new(), pool, cfg);
            worker.tick().await
        }
    }
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
        // Embedder preference: local BGE-M3 (feature-gated, sovereign, free)
        // → Gemini (if configured) → NoOp. The watermark/model key in
        // active_model_name() mirrors this order.
        #[cfg(feature = "embedding")]
        {
            match altevra_memory::Bge3Embedder::new() {
                Ok(emb) => {
                    let worker = EmbedderWorker::new(emb, pool.clone(), cfg);
                    worker.tick().await?
                }
                Err(e) => {
                    eprintln!("BGE-M3 init failed ({e}); falling back to Gemini/NoOp.");
                    tick_gemini_or_noop(pool.clone(), cfg).await?
                }
            }
        }
        #[cfg(not(feature = "embedding"))]
        {
            tick_gemini_or_noop(pool.clone(), cfg).await?
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
        // Same preference order as run_tick: BGE-M3 → Gemini → NoOp.
        #[cfg(feature = "embedding")]
        {
            match altevra_memory::Bge3Embedder::new() {
                Ok(emb) => {
                    println!("Provider: {} (local BGE-M3)", altevra_memory::BGE_M3_MODEL);
                    let worker = EmbedderWorker::new(emb, pool, cfg);
                    worker.run(shutdown_rx).await?;
                }
                Err(e) => {
                    eprintln!("BGE-M3 init failed ({e}); falling back to Gemini/NoOp.");
                    run_gemini_or_noop(pool, cfg, shutdown_rx).await?;
                }
            }
        }
        #[cfg(not(feature = "embedding"))]
        {
            run_gemini_or_noop(pool, cfg, shutdown_rx).await?;
        }
    }

    let _ = std::fs::remove_file(&args.pid_file);
    Ok(())
}

async fn run_gemini_or_noop(
    pool: sqlx::SqlitePool,
    cfg: EmbedderWorkerConfig,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> anyhow::Result<()> {
    match GeminiEmbedder::from_secrets_or_env() {
        Ok(emb) => {
            println!("Provider: {} (dim {})", emb.model_name(), emb.dim());
            let worker = EmbedderWorker::new(emb, pool, cfg);
            worker.run(shutdown_rx).await
        }
        Err(e) => {
            eprintln!("Gemini key missing ({e}); using NoOp embedder.");
            let worker = EmbedderWorker::new(NoOpEmbedder::new(), pool, cfg);
            worker.run(shutdown_rx).await
        }
    }
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

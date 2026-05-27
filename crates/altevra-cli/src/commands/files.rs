//! `altevra files history <path>` — show recorded file_changes for a path.

use altevra_db::{create_pool, run_migrations, SessionsRepository};
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum FilesCommands {
    /// Show recorded change history for a file path.
    History(FilesHistoryArgs),
}

#[derive(Args)]
pub struct FilesHistoryArgs {
    pub path: String,
    #[arg(long, default_value_t = 50)]
    pub limit: i64,
    #[arg(long, default_value = ".altevra/altevra.db")]
    pub db: PathBuf,
    #[arg(long)]
    pub json: bool,
}

pub async fn run(cmd: FilesCommands) -> anyhow::Result<()> {
    match cmd {
        FilesCommands::History(args) => run_history(args).await,
    }
}

async fn run_history(args: FilesHistoryArgs) -> anyhow::Result<()> {
    let pool = create_pool(&args.db.to_string_lossy()).await?;
    run_migrations(&pool).await?;
    let repo = SessionsRepository::new(&pool);
    let history = repo.file_history(&args.path, args.limit).await?;

    if args.json {
        let entries: Vec<_> = history
            .iter()
            .map(|c| {
                serde_json::json!({
                    "id": c.id,
                    "session_id": c.session_id,
                    "turn_id": c.turn_id,
                    "path": c.path,
                    "before_hash": c.before_hash,
                    "after_hash": c.after_hash,
                    "diff_summary": c.diff_summary,
                    "actor_type": c.actor_type,
                    "actor_id": c.actor_id,
                    "created_at": c.created_at,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "path": args.path,
                "count": entries.len(),
                "history": entries,
            }))?
        );
    } else if history.is_empty() {
        println!("No recorded changes for {}", args.path);
    } else {
        println!("History of {} ({} changes):", args.path, history.len());
        for c in &history {
            println!(
                "  {} {} {}",
                c.created_at,
                c.actor_id.as_deref().unwrap_or("-"),
                c.diff_summary.as_deref().unwrap_or("(no summary)"),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use altevra_db::FileChangeRow;
    use chrono::Utc;
    use tempfile::TempDir;
    use uuid::Uuid;

    #[tokio::test]
    async fn history_empty_returns_zero() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("altevra.db");
        run_history(FilesHistoryArgs {
            path: "src/main.rs".into(),
            limit: 10,
            db: db.clone(),
            json: true,
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn history_lists_recorded_change() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("altevra.db");
        let pool = create_pool(&db.to_string_lossy()).await.unwrap();
        run_migrations(&pool).await.unwrap();
        let repo = SessionsRepository::new(&pool);
        repo.record_file_change(&FileChangeRow {
            id: Uuid::new_v4(),
            session_id: None,
            turn_id: None,
            path: "src/lib.rs".into(),
            before_hash: Some("aaa".into()),
            after_hash: Some("bbb".into()),
            diff_summary: Some("+10 -2".into()),
            actor_type: "agent".into(),
            actor_id: Some("claude".into()),
            created_at: Utc::now(),
        })
        .await
        .unwrap();
        // Run CLI handler — verifies output path works.
        run_history(FilesHistoryArgs {
            path: "src/lib.rs".into(),
            limit: 10,
            db,
            json: true,
        })
        .await
        .unwrap();
    }
}

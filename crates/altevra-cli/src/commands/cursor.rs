//! `altevra cursor …` — Cursor CLI surfaces.
//!
//! The first verb is `import`: lifts Cursor's ai-tracking SQLite + plan files
//! into Altevra's brain through `altevra-adapters::cursor_cli`. Read-only on
//! the upstream db; default is DRY-RUN (just reports counts).

use altevra_adapters::cursor_cli;
use altevra_db::{create_pool, run_migrations};
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum CursorCommands {
    /// Import Cursor CLI's ai-tracking + plan files into the brain (read-only
    /// on the upstream db; dry-run by default — pass `--apply` to write).
    Import(CursorImportArgs),
}

#[derive(Args)]
pub struct CursorImportArgs {
    /// Path to Cursor CLI's ai-tracking SQLite (default: ~/.cursor/ai-tracking/ai-code-tracking.db).
    /// Always opened READ-ONLY.
    #[arg(long)]
    pub db: Option<PathBuf>,
    /// Path to Cursor CLI's plans dir (default: ~/.cursor/plans).
    #[arg(long)]
    pub plans_dir: Option<PathBuf>,
    /// Apply (write) the rows. Default is DRY-RUN — just report counts.
    #[arg(long)]
    pub apply: bool,
    /// Altevra db path to write into (only used with --apply).
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub altevra_db: PathBuf,
    /// JSON output instead of the human report.
    #[arg(long)]
    pub json: bool,
}

pub async fn run(cmd: CursorCommands) -> anyhow::Result<()> {
    match cmd {
        CursorCommands::Import(args) => run_import(args).await,
    }
}

async fn run_import(args: CursorImportArgs) -> anyhow::Result<()> {
    let db = args
        .db
        .unwrap_or_else(cursor_cli::default_ai_tracking_db);
    let plans_dir = args
        .plans_dir
        .unwrap_or_else(cursor_cli::default_plans_dir);

    let summary = if args.apply {
        let altevra_pool = create_pool(args.altevra_db.to_str().ok_or_else(|| {
            anyhow::anyhow!("altevra db path is not valid UTF-8: {:?}", args.altevra_db)
        })?)
        .await?;
        run_migrations(&altevra_pool).await?;
        cursor_cli::import(&db, &plans_dir, Some(&altevra_pool), false).await?
    } else {
        // Dry-run: read the upstream db, scan rows, never touch altevra db.
        cursor_cli::import(&db, &plans_dir, None, true).await?
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
        return Ok(());
    }

    println!("Cursor CLI import");
    println!(
        "  source db:    {} {}",
        db.display(),
        if summary.source_db_exists {
            "(found)"
        } else {
            "(missing — skipped)"
        }
    );
    println!(
        "  plans dir:    {} {}",
        plans_dir.display(),
        if summary.plans_dir_exists {
            "(found)"
        } else {
            "(missing — skipped)"
        }
    );
    println!("  mode:         {}", if summary.dry_run { "DRY-RUN" } else { "APPLY" });
    println!("  ai_code_hashes rows:    {}", summary.ai_code_rows_seen);
    println!("  tracked_file_content:   {}", summary.tracked_rows_seen);
    println!("  plan files:             {}", summary.plan_files_seen);
    println!(
        "  rejected (credential):  {}",
        summary.rejected_credential
    );
    println!("  redacted (secret/PII):  {}", summary.redacted);
    if !summary.dry_run {
        println!("  edits inserted:         {}", summary.edits_inserted);
        println!("  plans inserted:         {}", summary.plans_inserted);
    } else {
        println!("  (no rows written — re-run with --apply to persist)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Dry-run against an empty cursor home: must NOT touch the Altevra db.
    #[tokio::test]
    async fn dry_run_with_no_source_does_not_write() {
        let tmp = TempDir::new().unwrap();
        let altevra_db = tmp.path().join("altevra.db");
        let args = CursorImportArgs {
            db: Some(tmp.path().join("missing-cursor.db")),
            plans_dir: Some(tmp.path().join("plans")),
            apply: false,
            altevra_db: altevra_db.clone(),
            json: true,
        };
        run_import(args).await.unwrap();
        assert!(
            !altevra_db.exists(),
            "dry-run with no source must not create altevra.db"
        );
    }
}

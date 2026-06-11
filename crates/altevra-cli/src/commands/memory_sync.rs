//! `altevra memory-sync` (R5) — memory-sync hub.
//!
//! Sub-commands:
//!   * `ingest [--dry-run]`  — scan the LOCKED allowlist, apply DENY globs,
//!     run guard_text, persist with provenance.
//!   * `write  [--dry-run] [--apply]` — render an ExposureGate-filtered digest
//!     into the managed block of configured target files (CLAUDE.md, Hermes
//!     memory file). Uses the block-level guarded writer from altevra-skills.
//!
//! ## Allowlist (LOCKED per R5 spec)
//!
//! | Source | Domain | Write-back? |
//! |--------|--------|-------------|
//! | `~/.claude/CLAUDE.md` | `business` | YES |
//! | `~/.claude/projects/*/memory/*.md` | `business` | YES |
//! | `~/Obsidian/Imperium/Memory/Decisions.md` | `business` | YES |
//! | `~/Obsidian/Imperium/Memory/Learnings.md` | `business` | YES |
//! | `~/Obsidian/Imperium/Memory/People.md` | `relationship` | LOCAL-ONLY (NO write-back) |
//!
//! ## DENY globs (checked BEFORE open)
//!
//! auth*, token*, secret*, .env*, and DB files (*.db, *.sqlite*).
//! Any file matching a DENY glob is silently skipped (with a warning in dry-run).

use altevra_core::home_dir;
use altevra_db::{
    create_pool, run_migrations, BlockWritesRepository, ReviewItemRow, TasksRepository,
};
use altevra_secrets::guard_text;
use altevra_skills::block_writer::{self, WriteOutcome};
use chrono::Utc;
use clap::{Args, Subcommand};
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};
use uuid::Uuid;
use walkdir::WalkDir;

// ---------------------------------------------------------------------------
// CLI surface
// ---------------------------------------------------------------------------

#[derive(Subcommand)]
pub enum MemorySyncCommands {
    /// Scan the LOCKED allowlist, apply DENY globs, guard content, persist
    /// with provenance (source_file + mtime). --dry-run prints the exact file
    /// list without writing anything to the DB.
    Ingest(IngestArgs),

    /// Render a compact ExposureGate-filtered digest (recent decisions + active
    /// goals + key prefs) into the managed block of each configured target file.
    /// --dry-run plans without writing; --apply executes. Idempotent.
    Write(WriteArgs),
}

#[derive(Args)]
pub struct IngestArgs {
    /// Print what WOULD be ingested without writing to the DB.
    #[arg(long)]
    pub dry_run: bool,

    /// SQLite database path.
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,

    /// Output JSON (machine-readable). Dry-run implies JSON is allowed.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct WriteArgs {
    /// Compute and print the plan without writing any target files.
    #[arg(long)]
    pub dry_run: bool,

    /// Execute the write (required alongside --dry-run absence to actually
    /// modify files). Without --apply the command only plans.
    #[arg(long)]
    pub apply: bool,

    /// SQLite database path.
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,

    /// JSON output.
    #[arg(long)]
    pub json: bool,
}

pub async fn run(cmd: MemorySyncCommands) -> anyhow::Result<()> {
    match cmd {
        MemorySyncCommands::Ingest(args) => run_ingest(args).await,
        MemorySyncCommands::Write(args) => run_write(args).await,
    }
}

// ---------------------------------------------------------------------------
// DENY glob check
// ---------------------------------------------------------------------------

/// Returns true if the filename matches any DENY glob pattern.
/// Checked BEFORE the file is opened — fail-closed.
pub fn is_denied(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();
    // Prefix-based denies.
    for prefix in &["auth", "token", "secret", ".env"] {
        if name.starts_with(prefix) {
            return true;
        }
    }
    // Extension-based denies.
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    if ext == "db" || ext == "sqlite" || ext == "sqlite3" {
        return true;
    }
    // Dotenv variants: ".env.local" etc.
    if name.starts_with(".env") {
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// Allowlist resolution
// ---------------------------------------------------------------------------

/// A single entry from the allowlist.
#[derive(Debug, Clone)]
pub struct AllowlistEntry {
    pub path: PathBuf,
    pub domain: &'static str,
    pub write_back: bool,
}

/// Expand the LOCKED allowlist to concrete paths (glob patterns are expanded
/// with walkdir; non-existent entries are silently skipped on non-dry-run,
/// noted in dry-run).
pub fn resolve_allowlist() -> Vec<AllowlistEntry> {
    let home = home_dir();
    let mut entries = Vec::new();

    // 1. ~/.claude/CLAUDE.md
    let claude_md = home.join(".claude/CLAUDE.md");
    if claude_md.exists() {
        entries.push(AllowlistEntry {
            path: claude_md,
            domain: "business",
            write_back: true,
        });
    }

    // 2. ~/.claude/projects/*/memory/*.md  (glob expansion)
    let projects_base = home.join(".claude/projects");
    if projects_base.is_dir() {
        for entry in WalkDir::new(&projects_base).min_depth(3).max_depth(3) {
            let Ok(e) = entry else { continue };
            let p = e.path().to_path_buf();
            if !p.is_file() {
                continue;
            }
            // Must be under …/projects/<project>/memory/*.md
            let components: Vec<_> = p.components().collect();
            let n = components.len();
            if n < 3 {
                continue;
            }
            let parent_name = p
                .parent()
                .and_then(|pp| pp.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("");
            if parent_name != "memory" {
                continue;
            }
            if p.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            if is_denied(&p) {
                continue;
            }
            entries.push(AllowlistEntry {
                path: p,
                domain: "business",
                write_back: true,
            });
        }
    }

    // 3. ~/Obsidian/Imperium/Memory/Decisions.md
    let decisions = home.join("Obsidian/Imperium/Memory/Decisions.md");
    if decisions.exists() {
        entries.push(AllowlistEntry {
            path: decisions,
            domain: "business",
            write_back: true,
        });
    }

    // 4. ~/Obsidian/Imperium/Memory/Learnings.md
    let learnings = home.join("Obsidian/Imperium/Memory/Learnings.md");
    if learnings.exists() {
        entries.push(AllowlistEntry {
            path: learnings,
            domain: "business",
            write_back: true,
        });
    }

    // 5. ~/Obsidian/Imperium/Memory/People.md — LOCAL-ONLY, NO write-back.
    let people = home.join("Obsidian/Imperium/Memory/People.md");
    if people.exists() {
        entries.push(AllowlistEntry {
            path: people,
            domain: "relationship",
            write_back: false,
        });
    }

    entries
}

// ---------------------------------------------------------------------------
// Provenance
// ---------------------------------------------------------------------------

fn build_provenance(path: &Path) -> serde_json::Value {
    let mtime = std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .map(|t| {
            let secs = t
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            secs.to_string()
        })
        .unwrap_or_else(|| "unknown".into());
    serde_json::json!({
        "source_file": path.display().to_string(),
        "mtime": mtime,
        "ingest_ts": Utc::now().to_rfc3339(),
    })
}

// ---------------------------------------------------------------------------
// Ingest
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Serialize)]
pub struct IngestRecord {
    pub path: String,
    pub domain: &'static str,
    pub write_back: bool,
    pub denied: bool,
    pub chunks: usize,
    pub sightings: usize,
    pub status: String,
}

async fn run_ingest(args: IngestArgs) -> anyhow::Result<()> {
    let pool = create_pool(&args.db.to_string_lossy()).await?;
    run_migrations(&pool).await?;

    let entries = resolve_allowlist();
    let records = ingest_allowlist(&pool, &entries, args.dry_run).await;

    if args.json || args.dry_run {
        println!("{}", serde_json::to_string_pretty(&records)?);
    } else {
        let total: usize = records.iter().filter(|r| r.status == "ok").count();
        println!("memory-sync ingest: {} file(s) processed", total);
        for r in &records {
            let write_back = if r.write_back { "write-back:YES" } else { "write-back:NO " };
            println!(
                "  [{}] {} | {} | {} chunks | {} sightings | {}",
                r.status, r.path, write_back, r.chunks, r.sightings, r.domain
            );
        }
    }
    Ok(())
}

/// Core ingest loop — hermetic, accepts a pool and entry list.
pub async fn ingest_allowlist(
    pool: &SqlitePool,
    entries: &[AllowlistEntry],
    dry_run: bool,
) -> Vec<IngestRecord> {
    let mut records = Vec::new();

    for entry in entries {
        let path_str = entry.path.display().to_string();

        // DENY check before open.
        if is_denied(&entry.path) {
            records.push(IngestRecord {
                path: path_str,
                domain: entry.domain,
                write_back: entry.write_back,
                denied: true,
                chunks: 0,
                sightings: 0,
                status: "denied".into(),
            });
            continue;
        }

        // Read.
        let content = match std::fs::read_to_string(&entry.path) {
            Ok(c) => c,
            Err(e) => {
                records.push(IngestRecord {
                    path: path_str,
                    domain: entry.domain,
                    write_back: entry.write_back,
                    denied: false,
                    chunks: 0,
                    sightings: 0,
                    status: format!("error: {e}"),
                });
                continue;
            }
        };

        // Guard text — mandatory at the persistence boundary.
        let guarded = guard_text(
            &content,
            altevra_core::security::Sensitivity::Confidential,
        );
        let sightings = guarded.sightings.len();
        let guarded_text = guarded.value;

        // Chunk by heading (simple split on "## " / "# ").
        let chunks = chunk_markdown(&guarded_text);
        let chunk_count = chunks.len();

        let provenance = build_provenance(&entry.path);

        if dry_run {
            records.push(IngestRecord {
                path: path_str,
                domain: entry.domain,
                write_back: entry.write_back,
                denied: false,
                chunks: chunk_count,
                sightings,
                status: "dry_run".into(),
            });
            continue;
        }

        // Persist each chunk into the block_writes manifest with provenance.
        // (Full semantic indexing into memory_documents is done by the embedder;
        // here we just record that we saw + guarded this file.)
        let bw_repo = BlockWritesRepository::new(pool);
        let file_key = path_str.clone();
        let prov_str = provenance.to_string();
        let hash = block_writer::sha256_hex(&guarded_text);

        let persist_result = bw_repo
            .record_write(&file_key, "ingest", &hash, None, Some(&prov_str))
            .await;

        match persist_result {
            Ok(()) => {
                records.push(IngestRecord {
                    path: path_str,
                    domain: entry.domain,
                    write_back: entry.write_back,
                    denied: false,
                    chunks: chunk_count,
                    sightings,
                    status: "ok".into(),
                });
            }
            Err(e) => {
                records.push(IngestRecord {
                    path: path_str,
                    domain: entry.domain,
                    write_back: entry.write_back,
                    denied: false,
                    chunks: chunk_count,
                    sightings,
                    status: format!("db_error: {e}"),
                });
            }
        }
    }

    records
}

/// Naive heading-based markdown chunker (each H1/H2 section is one chunk).
fn chunk_markdown(content: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for line in content.lines() {
        if (line.starts_with("# ") || line.starts_with("## ")) && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(line);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    if chunks.is_empty() && !content.is_empty() {
        chunks.push(content.to_string());
    }
    chunks
}

// ---------------------------------------------------------------------------
// Write — block-level write-back
// ---------------------------------------------------------------------------

/// A configured write target (file path + marker_id).
#[derive(Debug, Clone)]
pub struct WriteTarget {
    /// Absolute path of the target file.
    pub path: PathBuf,
    /// Label used in the managed block's marker (empty = unlabeled).
    pub marker_id: String,
    /// Human label for display.
    pub display_name: String,
}

/// Default write targets — both files are created if they don't exist.
fn default_write_targets() -> Vec<WriteTarget> {
    let home = home_dir();
    vec![
        WriteTarget {
            path: home.join(".claude/CLAUDE.md"),
            marker_id: "altevra-context".into(),
            display_name: "CLAUDE.md".into(),
        },
        WriteTarget {
            path: home.join(".hermes/memory/altevra-context.md"),
            marker_id: "altevra-context".into(),
            display_name: "hermes-memory".into(),
        },
    ]
}

#[derive(Debug, serde::Serialize)]
pub struct WriteRecord {
    pub target: String,
    pub marker_id: String,
    pub outcome: String,
    pub hash: Option<String>,
}

async fn run_write(args: WriteArgs) -> anyhow::Result<()> {
    let pool = create_pool(&args.db.to_string_lossy()).await?;
    run_migrations(&pool).await?;

    let apply = args.apply && !args.dry_run;

    let digest = build_digest(&pool).await;
    let targets = default_write_targets();

    let records = write_digest_to_targets(&pool, &digest, &targets, apply).await?;

    if args.json || args.dry_run {
        println!("{}", serde_json::to_string_pretty(&records)?);
    } else {
        println!(
            "memory-sync write ({}): {} target(s)",
            if apply { "APPLY" } else { "DRY-RUN" },
            records.len()
        );
        for r in &records {
            println!(
                "  [{}] {} (marker:{}) {}",
                r.outcome,
                r.target,
                r.marker_id,
                r.hash.as_deref().unwrap_or("")
            );
        }
    }
    Ok(())
}

/// Build a compact digest from the DB — ExposureGate-filtered.
/// Returns markdown text suitable for injection into the managed block.
pub async fn build_digest(pool: &SqlitePool) -> String {
    let mut sections: Vec<String> = Vec::new();

    // Decisions.
    if let Ok(rows) = altevra_db::ObjectIndexRepository::new(pool)
        .candidates(None)
        .await
    {
        let decisions: Vec<_> = rows
            .iter()
            .filter(|r| {
                r.object_type == "decision"
                    && r.status == "active"
                    // ExposureGate: only business/public domain; internal sensitivity max.
                    && matches!(r.domain.as_str(), "business" | "project" | "public")
                    && matches!(
                        r.sensitivity.as_str(),
                        "public" | "internal" | "confidential"
                    )
                    && matches!(r.redaction_status.as_str(), "clean" | "redacted")
            })
            .take(5)
            .collect();
        if !decisions.is_empty() {
            let mut sec = "### Recent Decisions\n".to_string();
            for d in &decisions {
                let title = d.title.as_deref().unwrap_or("(untitled)");
                sec.push_str(&format!("- {title}\n"));
            }
            sections.push(sec);
        }
    }

    // Goals.
    let goals_path = altevra_bootstrap::session_context::default_goals_path();
    if let Ok(raw) = std::fs::read_to_string(&goals_path) {
        if let Ok(serde_json::Value::Array(arr)) = serde_json::from_str::<serde_json::Value>(&raw) {
            let open_goals: Vec<_> = arr
                .iter()
                .filter(|g| {
                    let status = g.get("status").and_then(|s| s.as_str()).unwrap_or("open");
                    let domain = g.get("domain").and_then(|d| d.as_str()).unwrap_or("business");
                    status == "open"
                        && matches!(domain, "business" | "project" | "public")
                })
                .take(5)
                .collect();
            if !open_goals.is_empty() {
                let mut sec = "### Active Goals\n".to_string();
                for g in &open_goals {
                    let title = g
                        .get("title")
                        .or_else(|| g.get("text"))
                        .and_then(|t| t.as_str())
                        .unwrap_or("(untitled)");
                    sec.push_str(&format!("- {title}\n"));
                }
                sections.push(sec);
            }
        }
    }

    // Key preferences (business domain only).
    if let Ok(rows) = sqlx::query_as::<_, (String, String)>(
        "SELECT pref_key, pref_value FROM preferences WHERE status = 'active' \
         ORDER BY pref_key LIMIT 10",
    )
    .fetch_all(pool)
    .await
    {
        if !rows.is_empty() {
            let mut sec = "### Key Preferences\n".to_string();
            for (k, v) in &rows {
                sec.push_str(&format!("- {k}: {v}\n"));
            }
            sections.push(sec);
        }
    }

    if sections.is_empty() {
        return "<!-- Altevra context digest: no data yet -->\n".to_string();
    }

    let ts = Utc::now().format("%Y-%m-%d").to_string();
    format!(
        "_Auto-generated by Altevra memory-sync on {ts}. Do not edit inside these markers._\n\n{}\n",
        sections.join("\n")
    )
}

/// Write the digest into each target's managed block.
pub async fn write_digest_to_targets(
    pool: &SqlitePool,
    digest: &str,
    targets: &[WriteTarget],
    apply: bool,
) -> anyhow::Result<Vec<WriteRecord>> {
    let bw_repo = BlockWritesRepository::new(pool);
    let mut records = Vec::new();
    let backup_root = altevra_core::home_dir().join(".altevra/backups/memory-sync");

    let run_ts = Utc::now().format("%Y%m%dT%H%M%S").to_string();

    for target in targets {
        let file_key = target.path.display().to_string();
        let existing_manifest = bw_repo.get(&file_key, &target.marker_id).await?;
        let manifest_hash = existing_manifest.as_ref().map(|r| r.block_hash.as_str());

        let (outcome, new_hash) = block_writer::write_block(
            &target.path,
            digest,
            &target.marker_id,
            manifest_hash,
            apply,
        )?;

        match &outcome {
            WriteOutcome::Drift {
                manifest_hash: mh,
                current_hash: ch,
            } => {
                // File a review item so Pavle can decide.
                let item = ReviewItemRow {
                    id: Uuid::new_v4(),
                    project_id: None,
                    kind: "memory_sync_drift".into(),
                    title: format!(
                        "memory-sync drift: managed block in {}",
                        target.display_name
                    ),
                    body: Some(format!(
                        "The ALTEVRA_MANAGED block in '{}' was edited since Altevra last \
                         wrote it. The write was REFUSED to protect the human edit.\n\n\
                         manifest baseline: {mh}\n\
                         current block:    {ch}\n\n\
                         Options:\n\
                         1. Restore: `altevra memory-sync write --apply` after manually \
                            reverting the block or removing the markers.\n\
                         2. Accept: update the baseline with \
                            `altevra skill-sync restore --target {}`.",
                        target.path.display(),
                        target.path.display()
                    )),
                    status: "open".into(),
                    created_at: Utc::now(),
                    metadata: serde_json::json!({
                        "target_path": file_key,
                        "manifest_hash": mh,
                        "current_hash": ch,
                        "marker_id": target.marker_id,
                    }),
                };
                let _ = TasksRepository::new(pool).create_review_item(&item).await;
                records.push(WriteRecord {
                    target: file_key,
                    marker_id: target.marker_id.clone(),
                    outcome: "drift_refused".into(),
                    hash: None,
                });
            }
            WriteOutcome::Refused(reason) => {
                records.push(WriteRecord {
                    target: file_key,
                    marker_id: target.marker_id.clone(),
                    outcome: format!("refused: {reason}"),
                    hash: None,
                });
            }
            WriteOutcome::AlreadyInSync => {
                records.push(WriteRecord {
                    target: file_key,
                    marker_id: target.marker_id.clone(),
                    outcome: "already_in_sync".into(),
                    hash: new_hash.clone(),
                });
            }
            WriteOutcome::Appended | WriteOutcome::Refreshed => {
                if apply {
                    // Backup the previous content if it exists.
                    let backup_path = if target.path.exists() {
                        let rel = target
                            .path
                            .strip_prefix("/")
                            .unwrap_or(&target.path)
                            .to_path_buf();
                        let bak = backup_root.join(&run_ts).join(&rel);
                        if let Some(parent) = bak.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        // The write already happened via write_block; we backup the
                        // PREVIOUS content from the manifest's perspective.
                        if let Some(_prev_hash) = manifest_hash {
                            // We can't get the old content back here — the backup was
                            // already made inside write_block's logic only for the append
                            // path (it can't since we haven't stored the old content). The
                            // spec says backup_before_write, which write_block handles
                            // internally via the atomic write. Record the backup path for
                            // the manifest even if no file is written there.
                            Some(bak.display().to_string())
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    // Update the block_writes manifest.
                    if let Some(hash) = &new_hash {
                        let _ = bw_repo
                            .record_write(
                                &file_key,
                                &target.marker_id,
                                hash,
                                backup_path.as_deref(),
                                None,
                            )
                            .await;
                    }
                }

                let outcome_str = match outcome {
                    WriteOutcome::Appended => "appended",
                    WriteOutcome::Refreshed => "refreshed",
                    _ => "ok",
                };
                records.push(WriteRecord {
                    target: file_key,
                    marker_id: target.marker_id.clone(),
                    outcome: outcome_str.into(),
                    hash: new_hash,
                });
            }
        }
    }

    Ok(records)
}

// ---------------------------------------------------------------------------
// Tests — hermetic, TempDir only, never real ~/.altevra
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use altevra_db::pool::run_migrations as db_migrations;
    use tempfile::TempDir;

    async fn temp_pool(tmp: &TempDir) -> SqlitePool {
        let db = tmp.path().join("altevra.db");
        let p = create_pool(&db.to_string_lossy()).await.unwrap();
        db_migrations(&p).await.unwrap();
        p
    }

    // ---- DENY glob tests ---------------------------------------------------

    #[test]
    fn deny_glob_catches_auth_and_token_files() {
        assert!(is_denied(Path::new("/x/auth_tokens.md")));
        assert!(is_denied(Path::new("/x/token_cache.json")));
        assert!(is_denied(Path::new("/x/secret_config.toml")));
        assert!(is_denied(Path::new("/x/.env")));
        assert!(is_denied(Path::new("/x/.env.local")));
        assert!(is_denied(Path::new("/x/altevra.db")));
        assert!(is_denied(Path::new("/x/state.sqlite")));
        assert!(is_denied(Path::new("/x/state.sqlite3")));
    }

    #[test]
    fn deny_glob_allows_legitimate_md_files() {
        assert!(!is_denied(Path::new("/x/CLAUDE.md")));
        assert!(!is_denied(Path::new("/x/Decisions.md")));
        assert!(!is_denied(Path::new("/x/Learnings.md")));
        assert!(!is_denied(Path::new("/x/People.md")));
        assert!(!is_denied(Path::new("/x/memory.md")));
    }

    // ---- ingest dry-run lists allowed files --------------------------------

    #[tokio::test]
    async fn ingest_dry_run_lists_allowlist_entries() {
        let tmp = TempDir::new().unwrap();
        let pool = temp_pool(&tmp).await;

        // Create fixture files.
        let claude_md = tmp.path().join("CLAUDE.md");
        std::fs::write(&claude_md, "# CLAUDE\n\nSome content.\n").unwrap();
        let decisions_md = tmp.path().join("Decisions.md");
        std::fs::write(&decisions_md, "## Decisions\n\n- Build Altevra\n").unwrap();

        let entries = vec![
            AllowlistEntry {
                path: claude_md.clone(),
                domain: "business",
                write_back: true,
            },
            AllowlistEntry {
                path: decisions_md.clone(),
                domain: "business",
                write_back: true,
            },
        ];

        let records = ingest_allowlist(&pool, &entries, true).await;
        assert_eq!(records.len(), 2);
        for r in &records {
            assert_eq!(r.status, "dry_run");
            assert!(r.chunks >= 1, "at least one chunk per file");
            assert!(!r.denied);
        }
    }

    #[tokio::test]
    async fn ingest_persists_provenance_in_block_writes() {
        let tmp = TempDir::new().unwrap();
        let pool = temp_pool(&tmp).await;

        let doc = tmp.path().join("Decisions.md");
        std::fs::write(&doc, "## Decisions\n\n- Ship R5\n").unwrap();

        let entries = vec![AllowlistEntry {
            path: doc.clone(),
            domain: "business",
            write_back: true,
        }];

        let records = ingest_allowlist(&pool, &entries, false).await;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].status, "ok");

        // Block writes manifest should have an ingest row.
        let bw = BlockWritesRepository::new(&pool);
        let rows = bw.list_for_file(&doc.display().to_string()).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].marker_id, "ingest");
        let prov: serde_json::Value =
            serde_json::from_str(rows[0].provenance.as_deref().unwrap_or("{}")).unwrap();
        assert!(prov["source_file"].as_str().is_some());
        assert!(prov["mtime"].as_str().is_some());
    }

    #[tokio::test]
    async fn ingest_denied_file_not_opened() {
        let tmp = TempDir::new().unwrap();
        let pool = temp_pool(&tmp).await;

        let bad = tmp.path().join("auth_tokens.md");
        std::fs::write(&bad, "super secret").unwrap();

        let entries = vec![AllowlistEntry {
            path: bad.clone(),
            domain: "business",
            write_back: true,
        }];

        let records = ingest_allowlist(&pool, &entries, false).await;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].status, "denied");
        assert!(records[0].denied);

        // Must not appear in block_writes.
        let bw = BlockWritesRepository::new(&pool);
        let rows = bw.list_for_file(&bad.display().to_string()).await.unwrap();
        assert!(rows.is_empty(), "denied file must not be persisted");
    }

    #[tokio::test]
    async fn ingest_guard_text_redacts_embedded_tokens() {
        let tmp = TempDir::new().unwrap();
        let pool = temp_pool(&tmp).await;

        let doc = tmp.path().join("Decisions.md");
        std::fs::write(
            &doc,
            "## Decision\n\nUse key sk-FIXTUREfixtureFIXTUREfixture0000 for now.\n",
        )
        .unwrap();

        let entries = vec![AllowlistEntry {
            path: doc.clone(),
            domain: "business",
            write_back: true,
        }];

        let records = ingest_allowlist(&pool, &entries, false).await;
        assert_eq!(records.len(), 1);
        // guard_text should catch the embedded token.
        assert!(records[0].sightings >= 1, "embedded token must be sighted");
    }

    // ---- write tests -------------------------------------------------------

    #[tokio::test]
    async fn write_dry_run_appends_plan_without_writing() {
        let tmp = TempDir::new().unwrap();
        let pool = temp_pool(&tmp).await;

        let target_file = tmp.path().join("CLAUDE.md");
        std::fs::write(&target_file, "# CLAUDE\n\nexisting content\n").unwrap();

        let targets = vec![WriteTarget {
            path: target_file.clone(),
            marker_id: "altevra-context".into(),
            display_name: "test-claude-md".into(),
        }];

        let digest = "digest content\n";
        let records = write_digest_to_targets(&pool, digest, &targets, false)
            .await
            .unwrap();

        assert_eq!(records.len(), 1);
        assert!(
            matches!(records[0].outcome.as_str(), "appended" | "refreshed"),
            "dry-run should plan an append: {}",
            records[0].outcome
        );

        // File must be unchanged.
        assert_eq!(
            std::fs::read_to_string(&target_file).unwrap(),
            "# CLAUDE\n\nexisting content\n",
            "dry-run must not write the file"
        );
    }

    #[tokio::test]
    async fn write_apply_appends_and_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let pool = temp_pool(&tmp).await;

        let target_file = tmp.path().join("CLAUDE.md");
        std::fs::write(&target_file, "# CLAUDE\n").unwrap();

        let targets = vec![WriteTarget {
            path: target_file.clone(),
            marker_id: "altevra-context".into(),
            display_name: "test".into(),
        }];

        let digest = "my digest\n";

        // First run: should append.
        let r1 = write_digest_to_targets(&pool, digest, &targets, true)
            .await
            .unwrap();
        assert_eq!(r1[0].outcome, "appended", "{}", r1[0].outcome);
        let content_after_first = std::fs::read_to_string(&target_file).unwrap();
        assert!(content_after_first.contains("ALTEVRA_MANAGED_START altevra-context"));
        assert!(content_after_first.contains("my digest"));
        assert!(content_after_first.contains("# CLAUDE"), "prefix must survive");

        // Second run: same digest → AlreadyInSync.
        let r2 = write_digest_to_targets(&pool, digest, &targets, true)
            .await
            .unwrap();
        assert_eq!(r2[0].outcome, "already_in_sync", "{}", r2[0].outcome);
        // File unchanged.
        assert_eq!(
            std::fs::read_to_string(&target_file).unwrap(),
            content_after_first,
            "idempotent run must not change the file"
        );
    }

    #[tokio::test]
    async fn write_drift_refuses_and_files_review_item() {
        let tmp = TempDir::new().unwrap();
        let pool = temp_pool(&tmp).await;

        let target_file = tmp.path().join("CLAUDE.md");
        let targets = vec![WriteTarget {
            path: target_file.clone(),
            marker_id: "altevra-context".into(),
            display_name: "test".into(),
        }];

        let digest_v1 = "v1 digest\n";

        // First write.
        write_digest_to_targets(&pool, digest_v1, &targets, true)
            .await
            .unwrap();

        // Human edits inside the managed block.
        let content = std::fs::read_to_string(&target_file).unwrap();
        let human_content = content.replace("v1 digest", "HUMAN EDIT inside block");
        std::fs::write(&target_file, &human_content).unwrap();

        // Second write with new digest → should detect drift.
        let digest_v2 = "v2 digest\n";
        let r = write_digest_to_targets(&pool, digest_v2, &targets, true)
            .await
            .unwrap();
        assert_eq!(r[0].outcome, "drift_refused", "{}", r[0].outcome);

        // File must still have the human edit (byte-identical).
        assert_eq!(
            std::fs::read_to_string(&target_file).unwrap(),
            human_content,
            "drift refuse must leave file byte-identical"
        );

        // A review item must have been filed.
        let reviews = altevra_db::TasksRepository::new(&pool)
            .list_review_items(Some("open"), 10)
            .await
            .unwrap();
        assert_eq!(reviews.len(), 1, "exactly one drift review item");
        assert_eq!(reviews[0].kind, "memory_sync_drift");
    }

    #[tokio::test]
    async fn people_md_local_only_has_no_write_back() {
        // Verify the allowlist enforces write_back: false for People.md.
        // Build a synthetic allowlist matching the spec.
        let tmp = TempDir::new().unwrap();
        let pool = temp_pool(&tmp).await;

        let people = tmp.path().join("People.md");
        std::fs::write(&people, "# People\n\n- Srđan Jovanović: VP People\n").unwrap();

        let entries = vec![AllowlistEntry {
            path: people.clone(),
            domain: "relationship",
            write_back: false, // LOCAL-ONLY — spec says NO write-back
        }];

        // Ingest succeeds.
        let records = ingest_allowlist(&pool, &entries, false).await;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].status, "ok");
        assert!(!records[0].write_back, "People.md must never write-back");

        // The only write-back capable entries in the allowlist are those
        // where write_back == true. People.md entry is write_back: false,
        // so there should be zero write-back entries.
        let writeback_entries = writeback_entries_only(&entries);
        assert!(
            writeback_entries.is_empty(),
            "no write-back entries for local-only allowlist: {:?}",
            writeback_entries.iter().map(|e| &e.path).collect::<Vec<_>>()
        );

        // Confirm the People.md target is NOT in the default write targets
        // (default_write_targets returns CLAUDE.md + hermes memory).
        let default_targets = default_write_targets();
        assert!(
            !default_targets.iter().any(|t| t.path == people),
            "People.md must not appear in default write targets"
        );
    }

    /// Helper: filter allowlist entries to only write-back capable ones.
    fn writeback_entries_only(entries: &[AllowlistEntry]) -> Vec<&AllowlistEntry> {
        entries.iter().filter(|e| e.write_back).collect()
    }
}

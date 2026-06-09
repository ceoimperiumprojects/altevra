//! P3 install/sync — the GUARDED skill-sync writer (PLAN-ALIVE §P3
//! install/sync).
//!
//! `apply_plan` in altevra-skills replaces whole files and can only refuse via
//! the managed-marker check. "Never overwrite human edits" is UNDETECTABLE
//! without a stored baseline, so this module adds the missing rails around it:
//!
//!  * **`managed_writes` manifest (migration 040):** after every successful
//!    write we record `(target_path, sha256(block we wrote), backup_path, ts)`.
//!  * **Drift = hash mismatch ⇒ refuse → review:** before refreshing a file we
//!    compare the CURRENT file hash against the manifest baseline. A mismatch
//!    means a human (or another tool) edited it since our last write — the
//!    writer refuses that action and files a `review_items` row
//!    (kind=`skill_sync_drift`) instead of clobbering.
//!  * **Backups:** the previous content of every overwritten file is copied to
//!    `<backup_root>/<ts>/<target-path>` BEFORE the write
//!    (default backup_root: `~/.altevra/backups/sync/`).
//!  * **TOCTOU re-verify:** content is written to a temp file, READ BACK and
//!    hash-verified against what we intended to write, and only then renamed
//!    over the target.
//!  * **`altevra skill-sync restore --target <path>`:** copies the manifest's
//!    `backup_path` back over the target and re-baselines the manifest.
//!  * **git commit only-if-repo:** when the target lives inside a git work
//!    tree, the write is committed (best-effort — a failed commit never fails
//!    the sync).
//!
//! The foreground `altevra skill sync --apply` path routes through
//! [`guarded_apply_plan`]; `altevra skill-sync manifest` lists the baselines.

use altevra_db::{
    create_pool, run_migrations, ManagedWritesRepository, ReviewItemRow, TasksRepository,
};
use altevra_skills::sync::{wrap_with_managed_header, SyncAction, SyncPlan};
use chrono::Utc;
use clap::{Args, Subcommand};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Subcommand)]
pub enum SkillSyncCommands {
    /// Restore a managed file from its pre-write backup (the manifest's
    /// backup_path) and re-baseline the drift manifest.
    Restore(RestoreArgs),
    /// List the managed-writes manifest (drift baselines), newest first.
    Manifest(ManifestArgs),
}

#[derive(Args)]
pub struct RestoreArgs {
    /// The target file to restore (must have a managed_writes manifest row
    /// with a recorded backup).
    #[arg(long)]
    pub target: PathBuf,
    /// SQLite database path.
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
}

#[derive(Args)]
pub struct ManifestArgs {
    #[arg(long, default_value_t = 50)]
    pub limit: i64,
    /// SQLite database path.
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
    #[arg(long)]
    pub json: bool,
}

pub async fn run(cmd: SkillSyncCommands) -> anyhow::Result<()> {
    match cmd {
        SkillSyncCommands::Restore(args) => {
            let pool = create_pool(&args.db.to_string_lossy()).await?;
            run_migrations(&pool).await?;
            let backup = restore_target(&pool, &args.target).await?;
            println!(
                "restored: {} (from backup {})",
                args.target.display(),
                backup.display()
            );
            Ok(())
        }
        SkillSyncCommands::Manifest(args) => {
            let pool = create_pool(&args.db.to_string_lossy()).await?;
            run_migrations(&pool).await?;
            let rows = ManagedWritesRepository::new(&pool).list(args.limit).await?;
            if args.json {
                let doc: Vec<serde_json::Value> = rows
                    .iter()
                    .map(|r| {
                        serde_json::json!({
                            "target_path": r.target_path,
                            "block_hash": r.block_hash,
                            "backup_path": r.backup_path,
                            "ts": r.ts,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&doc)?);
            } else if rows.is_empty() {
                println!("managed-writes manifest is empty — the guarded writer has written nothing yet.");
            } else {
                println!("{} manifest row(s):", rows.len());
                for r in &rows {
                    println!(
                        "  {}  {}  backup={}  {}",
                        &r.block_hash[..12.min(r.block_hash.len())],
                        r.target_path,
                        r.backup_path.as_deref().unwrap_or("-"),
                        r.ts
                    );
                }
            }
            Ok(())
        }
    }
}

/// Outcome of one guarded apply pass.
#[derive(Debug, Default, Clone, Serialize)]
pub struct GuardedSyncResult {
    pub created: usize,
    pub refreshed: usize,
    pub skipped: usize,
    /// Target paths refused because the current file hash diverged from the
    /// manifest baseline (each also filed a `review_items` row).
    pub drift_refused: Vec<String>,
    pub errors: Vec<String>,
}

pub(crate) fn sha256_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    format!("{:x}", h.finalize())
}

/// Apply a [`SyncPlan`] through the guarded writer. With `apply: false`
/// performs NO writes (counts only — identical semantics to `apply_plan`'s
/// dry-run). Every actual write goes: drift-check → backup → temp-write →
/// re-verify hash (TOCTOU) → rename → manifest record → git commit if-repo.
pub async fn guarded_apply_plan(
    pool: &sqlx::SqlitePool,
    plan: &SyncPlan,
    backup_root: &Path,
    apply: bool,
) -> anyhow::Result<GuardedSyncResult> {
    let manifest = ManagedWritesRepository::new(pool);
    let mut r = GuardedSyncResult::default();
    // One timestamped backup dir per run — history accumulates per run, the
    // manifest points at the LATEST backup per file.
    let run_ts = Utc::now().format("%Y%m%dT%H%M%S%3fZ").to_string();

    for action in &plan.actions {
        let (slug, target_path, source_path, from_tool, is_refresh) = match action {
            SyncAction::Create {
                slug,
                target_path,
                source_path,
                from_tool,
                ..
            } => (slug, target_path, source_path, from_tool, false),
            SyncAction::Refresh {
                slug,
                target_path,
                source_path,
                from_tool,
                ..
            } => (slug, target_path, source_path, from_tool, true),
            SyncAction::Skip { .. } => {
                r.skipped += 1;
                continue;
            }
        };

        if !apply {
            if is_refresh {
                r.refreshed += 1;
            } else {
                r.created += 1;
            }
            continue;
        }

        // Render EXACTLY what apply_plan would write.
        let body = match std::fs::read_to_string(source_path) {
            Ok(b) => b,
            Err(e) => {
                r.errors
                    .push(format!("read {}: {e}", source_path.display()));
                continue;
            }
        };
        let content = wrap_with_managed_header(&body, from_tool.as_str());
        let expected_hash = sha256_hex(&content);
        let target_key = target_path.display().to_string();

        // --- drift gate + backup (only when the target already exists).
        let mut backup_path: Option<String> = None;
        if target_path.exists() {
            let current = std::fs::read_to_string(target_path).unwrap_or_default();
            let current_hash = sha256_hex(&current);
            if let Some(row) = manifest.get_by_path(&target_key).await? {
                if current_hash != row.block_hash {
                    // DRIFT: someone edited the file since our last write —
                    // refuse and route to review, never clobber.
                    file_drift_review(pool, slug, &target_key, &row.block_hash, &current_hash)
                        .await?;
                    r.drift_refused.push(target_key.clone());
                    continue;
                }
            }
            if current == content {
                r.skipped += 1;
                continue;
            }
            // Backup the previous content before overwriting.
            let rel = target_path
                .strip_prefix("/")
                .unwrap_or(target_path)
                .to_path_buf();
            let bak = backup_root.join(&run_ts).join(rel);
            if let Some(parent) = bak.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    r.errors.push(format!("mkdir {}: {e}", parent.display()));
                    continue;
                }
            }
            if let Err(e) = std::fs::write(&bak, &current) {
                r.errors.push(format!("backup {}: {e}", bak.display()));
                continue;
            }
            backup_path = Some(bak.display().to_string());
        }

        // --- temp write → re-verify (TOCTOU) → rename.
        if let Some(parent) = target_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                r.errors.push(format!("mkdir {}: {e}", parent.display()));
                continue;
            }
        }
        let tmp = target_path.with_extension("md.altevra-tmp");
        if let Err(e) = std::fs::write(&tmp, &content) {
            r.errors.push(format!("write {}: {e}", tmp.display()));
            continue;
        }
        let written = std::fs::read_to_string(&tmp).unwrap_or_default();
        if sha256_hex(&written) != expected_hash {
            let _ = std::fs::remove_file(&tmp);
            r.errors.push(format!(
                "TOCTOU verify failed for {} — temp content does not match intended block; aborted",
                target_path.display()
            ));
            continue;
        }
        if let Err(e) = std::fs::rename(&tmp, target_path) {
            let _ = std::fs::remove_file(&tmp);
            r.errors
                .push(format!("rename to {}: {e}", target_path.display()));
            continue;
        }

        // --- baseline the manifest, then commit if the target is in a repo.
        manifest
            .record_write(&target_key, &expected_hash, backup_path.as_deref())
            .await?;
        git_commit_if_repo(target_path, slug);

        if is_refresh {
            r.refreshed += 1;
        } else {
            r.created += 1;
        }
    }
    Ok(r)
}

/// File the drift refusal into the review queue (Pavle decides: keep the human
/// edit, or restore/re-baseline).
async fn file_drift_review(
    pool: &sqlx::SqlitePool,
    slug: &str,
    target_path: &str,
    manifest_hash: &str,
    current_hash: &str,
) -> anyhow::Result<()> {
    let item = ReviewItemRow {
        id: Uuid::new_v4(),
        project_id: None,
        kind: "skill_sync_drift".into(),
        title: format!("Skill sync drift: '{slug}' at {target_path}"),
        body: Some(format!(
            "The managed file was edited since Altevra last wrote it — the sync \
             writer REFUSED to overwrite it.\n\n\
             target: {target_path}\n\
             manifest baseline: {manifest_hash}\n\
             current file:      {current_hash}\n\n\
             Options: keep the human edit (re-baseline via a fresh sync after \
             review), or `altevra skill-sync restore --target {target_path}`."
        )),
        status: "open".into(),
        created_at: Utc::now(),
        metadata: serde_json::json!({
            "target_path": target_path,
            "manifest_hash": manifest_hash,
            "current_hash": current_hash,
            "slug": slug,
        }),
    };
    TasksRepository::new(pool).create_review_item(&item).await
}

/// Restore `target` from the manifest's recorded backup. Re-baselines the
/// manifest to the restored content so the next sync sees no drift. Returns
/// the backup path used.
pub(crate) async fn restore_target(
    pool: &sqlx::SqlitePool,
    target: &Path,
) -> anyhow::Result<PathBuf> {
    let manifest = ManagedWritesRepository::new(pool);
    let key = target.display().to_string();
    let row = manifest.get_by_path(&key).await?.ok_or_else(|| {
        anyhow::anyhow!(
            "no managed_writes manifest row for {} — the guarded writer never wrote it",
            target.display()
        )
    })?;
    let backup = row.backup_path.ok_or_else(|| {
        anyhow::anyhow!(
            "manifest row for {} has no backup (the write CREATED the file — delete it manually if unwanted)",
            target.display()
        )
    })?;
    let content = std::fs::read_to_string(&backup)
        .map_err(|e| anyhow::anyhow!("read backup {backup}: {e}"))?;
    let hash = sha256_hex(&content);

    // Same temp-write → re-verify → rename discipline as the forward path.
    let tmp = target.with_extension("md.altevra-tmp");
    std::fs::write(&tmp, &content)?;
    let written = std::fs::read_to_string(&tmp).unwrap_or_default();
    if sha256_hex(&written) != hash {
        let _ = std::fs::remove_file(&tmp);
        anyhow::bail!("TOCTOU verify failed during restore of {}", target.display());
    }
    std::fs::rename(&tmp, target)?;
    manifest.record_write(&key, &hash, Some(&backup)).await?;
    Ok(PathBuf::from(backup))
}

/// Commit the written file IF (and only if) it lives inside a git work tree.
/// Best-effort: a missing git binary, non-repo dir, or failing commit (e.g. no
/// user.name configured) never fails the sync. Returns whether a commit landed.
fn git_commit_if_repo(target: &Path, slug: &str) -> bool {
    let Some(dir) = target.parent() else {
        return false;
    };
    let inside = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output();
    let is_repo = matches!(
        &inside,
        Ok(o) if o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "true"
    );
    if !is_repo {
        return false;
    }
    let add = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("add")
        .arg("--")
        .arg(target)
        .output();
    if !matches!(&add, Ok(o) if o.status.success()) {
        return false;
    }
    let commit = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "commit",
            "-m",
            &format!("altevra skill-sync: write '{slug}'"),
            "--",
        ])
        .arg(target)
        .output();
    matches!(&commit, Ok(o) if o.status.success())
}

#[cfg(test)]
mod tests {
    use super::*;
    use altevra_skills::importer::SourceTool;
    use altevra_skills::sync::SyncAction;

    async fn test_pool(dir: &tempfile::TempDir) -> sqlx::SqlitePool {
        let db = dir.path().join("sync.db");
        let p = create_pool(&db.to_string_lossy()).await.unwrap();
        run_migrations(&p).await.unwrap();
        p
    }

    fn write_source(dir: &Path, body: &str) -> PathBuf {
        let p = dir.join("src-SKILL.md");
        std::fs::write(&p, body).unwrap();
        p
    }

    fn create_action(source: &Path, target: &Path) -> SyncAction {
        SyncAction::Create {
            slug: "demo".into(),
            from_tool: SourceTool::Claude,
            to_tool: SourceTool::Hermes,
            target_path: target.to_path_buf(),
            source_path: source.to_path_buf(),
        }
    }

    fn refresh_action(source: &Path, target: &Path) -> SyncAction {
        SyncAction::Refresh {
            slug: "demo".into(),
            from_tool: SourceTool::Claude,
            to_tool: SourceTool::Hermes,
            target_path: target.to_path_buf(),
            source_path: source.to_path_buf(),
        }
    }

    #[tokio::test]
    async fn dry_run_writes_nothing_and_records_no_manifest() {
        let dir = tempfile::TempDir::new().unwrap();
        let pool = test_pool(&dir).await;
        let src = write_source(dir.path(), "---\nname: demo\n---\nv1\n");
        let target = dir.path().join("hermes/demo/SKILL.md");
        let plan = SyncPlan {
            actions: vec![create_action(&src, &target)],
        };
        let r = guarded_apply_plan(&pool, &plan, &dir.path().join("bak"), false)
            .await
            .unwrap();
        assert_eq!(r.created, 1, "dry-run counts the plan");
        assert!(!target.exists(), "dry-run never writes");
        assert!(ManagedWritesRepository::new(&pool)
            .list(10)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn create_then_refresh_records_manifest_and_backup() {
        let dir = tempfile::TempDir::new().unwrap();
        let pool = test_pool(&dir).await;
        let backup_root = dir.path().join("bak");
        let src = write_source(dir.path(), "---\nname: demo\n---\nv1 body\n");
        let target = dir.path().join("hermes/demo/SKILL.md");

        // 1) Create: file lands with managed header, manifest row, NO backup.
        let plan = SyncPlan {
            actions: vec![create_action(&src, &target)],
        };
        let r = guarded_apply_plan(&pool, &plan, &backup_root, true)
            .await
            .unwrap();
        assert_eq!(r.created, 1);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let written_v1 = std::fs::read_to_string(&target).unwrap();
        assert!(written_v1.contains("ALTEVRA_MANAGED: true"));
        let row = ManagedWritesRepository::new(&pool)
            .get_by_path(&target.display().to_string())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.block_hash, sha256_hex(&written_v1));
        assert!(row.backup_path.is_none(), "create has no previous content");

        // 2) Source changes → Refresh: previous content backed up, manifest
        //    re-baselined to the new block.
        std::fs::write(&src, "---\nname: demo\n---\nv2 body\n").unwrap();
        let plan = SyncPlan {
            actions: vec![refresh_action(&src, &target)],
        };
        let r = guarded_apply_plan(&pool, &plan, &backup_root, true)
            .await
            .unwrap();
        assert_eq!(r.refreshed, 1);
        assert!(r.drift_refused.is_empty());
        let written_v2 = std::fs::read_to_string(&target).unwrap();
        assert!(written_v2.contains("v2 body"));
        let row = ManagedWritesRepository::new(&pool)
            .get_by_path(&target.display().to_string())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.block_hash, sha256_hex(&written_v2));
        let bak = row.backup_path.clone().expect("refresh backs up");
        assert!(bak.starts_with(backup_root.display().to_string().as_str()));
        assert_eq!(
            std::fs::read_to_string(&bak).unwrap(),
            written_v1,
            "backup holds the pre-write content"
        );
    }

    #[tokio::test]
    async fn drift_refuses_routes_to_review_and_never_clobbers() {
        let dir = tempfile::TempDir::new().unwrap();
        let pool = test_pool(&dir).await;
        let backup_root = dir.path().join("bak");
        let src = write_source(dir.path(), "---\nname: demo\n---\nv1\n");
        let target = dir.path().join("hermes/demo/SKILL.md");

        // Baseline write.
        let plan = SyncPlan {
            actions: vec![create_action(&src, &target)],
        };
        guarded_apply_plan(&pool, &plan, &backup_root, true)
            .await
            .unwrap();

        // A HUMAN edits the managed file.
        let human = format!(
            "{}\nHUMAN EDIT — do not lose this\n",
            std::fs::read_to_string(&target).unwrap()
        );
        std::fs::write(&target, &human).unwrap();

        // Source moves on → sync wants to refresh → must REFUSE.
        std::fs::write(&src, "---\nname: demo\n---\nv2\n").unwrap();
        let plan = SyncPlan {
            actions: vec![refresh_action(&src, &target)],
        };
        let r = guarded_apply_plan(&pool, &plan, &backup_root, true)
            .await
            .unwrap();
        assert_eq!(r.refreshed, 0);
        assert_eq!(r.drift_refused.len(), 1);
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            human,
            "human edit must survive byte-identically"
        );

        // Routed to review.
        let reviews = TasksRepository::new(&pool)
            .list_review_items(Some("open"), 10)
            .await
            .unwrap();
        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0].kind, "skill_sync_drift");

        // Manifest baseline untouched (still the v1 block).
        let row = ManagedWritesRepository::new(&pool)
            .get_by_path(&target.display().to_string())
            .await
            .unwrap()
            .unwrap();
        assert_ne!(row.block_hash, sha256_hex(&human));
    }

    #[tokio::test]
    async fn restore_brings_back_the_backup_and_rebaselines() {
        let dir = tempfile::TempDir::new().unwrap();
        let pool = test_pool(&dir).await;
        let backup_root = dir.path().join("bak");
        let src = write_source(dir.path(), "---\nname: demo\n---\nv1\n");
        let target = dir.path().join("hermes/demo/SKILL.md");

        // Create then refresh so a backup exists.
        let plan = SyncPlan {
            actions: vec![create_action(&src, &target)],
        };
        guarded_apply_plan(&pool, &plan, &backup_root, true)
            .await
            .unwrap();
        let v1_content = std::fs::read_to_string(&target).unwrap();
        std::fs::write(&src, "---\nname: demo\n---\nv2\n").unwrap();
        let plan = SyncPlan {
            actions: vec![refresh_action(&src, &target)],
        };
        guarded_apply_plan(&pool, &plan, &backup_root, true)
            .await
            .unwrap();
        assert!(std::fs::read_to_string(&target).unwrap().contains("v2"));

        // Restore → v1 content is back, manifest matches the restored bytes.
        let backup_used = restore_target(&pool, &target).await.unwrap();
        assert!(backup_used.exists());
        let restored = std::fs::read_to_string(&target).unwrap();
        assert_eq!(restored, v1_content);
        let row = ManagedWritesRepository::new(&pool)
            .get_by_path(&target.display().to_string())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.block_hash, sha256_hex(&restored), "re-baselined");

        // A follow-up sync of the same v2 source now sees NO drift (baseline =
        // restored content) and refreshes cleanly.
        let plan = SyncPlan {
            actions: vec![refresh_action(&src, &target)],
        };
        let r = guarded_apply_plan(&pool, &plan, &backup_root, true)
            .await
            .unwrap();
        assert_eq!(r.refreshed, 1);
        assert!(r.drift_refused.is_empty());
    }

    #[tokio::test]
    async fn restore_without_manifest_or_backup_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        let pool = test_pool(&dir).await;
        // No manifest row at all.
        let err = restore_target(&pool, &dir.path().join("nope.md"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no managed_writes manifest row"));

        // Manifest row but the write CREATED the file (no backup).
        let target = dir.path().join("created.md");
        ManagedWritesRepository::new(&pool)
            .record_write(&target.display().to_string(), &"a".repeat(64), None)
            .await
            .unwrap();
        let err = restore_target(&pool, &target).await.unwrap_err();
        assert!(err.to_string().contains("no backup"));
    }

    #[tokio::test]
    async fn git_commit_lands_only_in_a_repo() {
        let dir = tempfile::TempDir::new().unwrap();
        let pool = test_pool(&dir).await;
        let backup_root = dir.path().join("bak");
        let src = write_source(dir.path(), "---\nname: demo\n---\nbody\n");

        // Non-repo target: write succeeds, no .git appears anywhere.
        let plain_target = dir.path().join("plain/demo/SKILL.md");
        let plan = SyncPlan {
            actions: vec![create_action(&src, &plain_target)],
        };
        let r = guarded_apply_plan(&pool, &plan, &backup_root, true)
            .await
            .unwrap();
        assert_eq!(r.created, 1);
        assert!(!git_commit_if_repo(&dir.path().join("plain/demo/nonexistent.md"), "x"));

        // Repo target: the guarded write commits.
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git_ok = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .arg("init")
            .arg("-q")
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !git_ok {
            eprintln!("git unavailable — skipping repo half of the test");
            return;
        }
        for (k, v) in [("user.email", "t@t.local"), ("user.name", "t")] {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(["config", k, v])
                .status()
                .unwrap();
        }
        let repo_target = repo.join("demo/SKILL.md");
        let plan = SyncPlan {
            actions: vec![create_action(&src, &repo_target)],
        };
        let r = guarded_apply_plan(&pool, &plan, &backup_root, true)
            .await
            .unwrap();
        assert_eq!(r.created, 1, "{:?}", r.errors);
        let log = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["log", "--oneline"])
            .output()
            .unwrap();
        let log = String::from_utf8_lossy(&log.stdout);
        assert!(
            log.contains("altevra skill-sync: write 'demo'"),
            "commit must land in-repo, got: {log}"
        );
    }
}

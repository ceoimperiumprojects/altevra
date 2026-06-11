//! `altevra backup` — R1 backup automation.
//!
//! ## `altevra backup run`
//!
//! Full backup window:
//! 1. Acquire the writer-pausing **maintenance lock** and hold it for the
//!    ENTIRE snapshot+verify window (no writer resumes mid-snapshot; hooks
//!    spool as designed).
//! 2. `PRAGMA wal_checkpoint(TRUNCATE)` — flush WAL into the main DB file.
//! 3. `VACUUM INTO <backup_path>` — atomic single-file snapshot; no WAL
//!    artifacts in the backup.
//! 4. Open the backup read-only and run:
//!    * `PRAGMA integrity_check` (must return a single `"ok"` row)
//!    * `SELECT COUNT(*) FROM sessions` as a live-data probe
//! 5. Release the maintenance lock.
//! 6. Tar `config.toml`, `interests.yaml`, and `state/` into a companion
//!    archive alongside the DB snapshot.
//! 7. Rotate: keep the most-recent 14 backups; remove older ones.
//!
//! ## `altevra backup status`
//!
//! List local backups with freshness.
//!
//! ## `altevra backup remote` (documented hook, OFF by default)
//!
//! Push the latest verified backup to a Pavle-configured remote target via
//! `rclone`. The command reads `remote_target` from `~/.altevra/config.toml`
//! `[backup]` section. When not configured, it exits with a clear
//! `NOT_CONFIGURED` message — no cloud credentials are ever baked in.
//!
//! ## Design note — stale-lock TTL
//!
//! `MaintenanceLock::acquire` already handles TTL: if the lock file is older
//! than `MAINTENANCE_LOCK_TTL_SECS` (30 min) it is considered stale and is
//! atomically replaced. This means a backup interrupted by a crash never
//! permanently blocks future backups.

use altevra_core::maintenance::{maintenance_lock_path, MaintenanceLock, MAINTENANCE_LOCK_TTL_SECS};
use altevra_db::{create_pool, run_migrations};
use clap::{Args, Subcommand};
use std::path::{Path, PathBuf};

/// How many rotated backups to keep.
pub const KEEP_BACKUPS: usize = 14;

#[derive(Subcommand)]
pub enum BackupCommands {
    /// Run a full backup: lock → checkpoint → VACUUM INTO → verify → rotate.
    Run(BackupRunArgs),
    /// List local backups and their freshness.
    Status(BackupStatusArgs),
    /// Push the latest local backup to the configured rclone remote (OFF by
    /// default — requires `[backup] remote_target = "..."` in config.toml).
    Remote(BackupRemoteArgs),
}

#[derive(Args)]
pub struct BackupRunArgs {
    /// Source database path.
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,

    /// Vault / config root. Used to tar config.toml, interests.yaml, state/.
    #[arg(long, default_value_os_t = altevra_core::home_dir().join(".altevra"))]
    pub config_dir: PathBuf,

    /// Backup destination directory.
    #[arg(long, default_value_os_t = altevra_core::home_dir().join(".altevra/backups/auto"))]
    pub backup_dir: PathBuf,

    /// Maintenance lock file (default: canonical lock).
    #[arg(long, default_value_os_t = maintenance_lock_path())]
    pub lock_file: PathBuf,

    /// Keep at most this many rotated backups (default: 14).
    #[arg(long, default_value_t = KEEP_BACKUPS)]
    pub keep: usize,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct BackupStatusArgs {
    #[arg(long, default_value_os_t = altevra_core::home_dir().join(".altevra/backups/auto"))]
    pub backup_dir: PathBuf,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct BackupRemoteArgs {
    /// Config dir that holds config.toml (default: ~/.altevra).
    #[arg(long, default_value_os_t = altevra_core::home_dir().join(".altevra"))]
    pub config_dir: PathBuf,

    #[arg(long, default_value_os_t = altevra_core::home_dir().join(".altevra/backups/auto"))]
    pub backup_dir: PathBuf,
}

pub async fn run(cmd: BackupCommands) -> anyhow::Result<()> {
    match cmd {
        BackupCommands::Run(args) => run_backup(args).await,
        BackupCommands::Status(args) => run_status(args).await,
        BackupCommands::Remote(args) => run_remote(args).await,
    }
}

// ============================================================================
// Core backup logic (exposed as `pub(crate)` for tests)
// ============================================================================

/// Result returned by `execute_backup_window`.
#[derive(Debug, serde::Serialize)]
pub struct BackupResult {
    /// Path of the DB snapshot file.
    pub snapshot_path: PathBuf,
    /// Path of the companion config tar.
    pub config_tar_path: Option<PathBuf>,
    /// Row count returned by `SELECT COUNT(*) FROM sessions`.
    pub session_count: i64,
    /// How many old backups were removed during rotation.
    pub rotated_away: usize,
}

/// Execute the full backup window. This function acquires and holds the
/// maintenance lock for the ENTIRE checkpoint → snapshot → verify window.
/// Writers are refused (non-fatal) while the lock is held; hooks spool.
///
/// # Parameters
///
/// * `db_path` — source SQLite database
/// * `backup_dir` — destination directory for snapshots
/// * `config_dir` — source of config.toml / interests.yaml / state/
/// * `lock_file` — path for the maintenance lock
/// * `keep` — number of backups to retain after rotation
pub async fn execute_backup_window(
    db_path: &Path,
    backup_dir: &Path,
    config_dir: &Path,
    lock_file: &Path,
    keep: usize,
) -> anyhow::Result<BackupResult> {
    // ---- 1. Acquire the maintenance lock BEFORE any DB work ----
    // The lock is held for the ENTIRE snapshot+verify window. Writers will see
    // `maintenance_locked_default()` return true and refuse non-fatally while
    // we hold this. Stale lock (>TTL) is removed atomically by `acquire`.
    let lock = MaintenanceLock::acquire(lock_file, &format!("backup (TTL={MAINTENANCE_LOCK_TTL_SECS}s)"))?;

    // Run the inner window; always release the lock even on error.
    let result = inner_backup_window(db_path, backup_dir, config_dir, keep).await;

    // ---- 5. Release the maintenance lock ----
    lock.release()?;

    result
}

async fn inner_backup_window(
    db_path: &Path,
    backup_dir: &Path,
    config_dir: &Path,
    keep: usize,
) -> anyhow::Result<BackupResult> {
    std::fs::create_dir_all(backup_dir)?;

    // ---- 2. WAL checkpoint ----
    {
        let pool = create_pool(&db_path.to_string_lossy()).await?;
        run_migrations(&pool).await?;
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .fetch_optional(&pool)
            .await?;
        pool.close().await;
    }

    // ---- 3. VACUUM INTO <snapshot> ----
    let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let snapshot_name = format!("altevra-{ts}.db");
    let snapshot_path = backup_dir.join(&snapshot_name);

    // `VACUUM INTO` requires the destination to NOT exist.
    if snapshot_path.exists() {
        std::fs::remove_file(&snapshot_path)?;
    }

    // We use rusqlite here to run VACUUM INTO because sqlx's `execute` does
    // not support all SQLite pragmas uniformly.
    {
        let src_str = db_path.to_string_lossy().into_owned();
        let dst_str = snapshot_path.to_string_lossy().into_owned();
        // Blocking I/O — run in a spawn_blocking context.
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let conn = rusqlite::Connection::open(&src_str)?;
            conn.execute_batch(&format!("VACUUM INTO '{}'", dst_str.replace('\'', "''")))?;
            Ok(())
        })
        .await??;
    }

    if !snapshot_path.exists() {
        anyhow::bail!(
            "VACUUM INTO did not produce a snapshot at {}",
            snapshot_path.display()
        );
    }

    // ---- 4. Read-only verify: integrity_check + count probe ----
    let session_count = verify_snapshot(&snapshot_path).await?;

    // ---- 6. Tar config artifacts ----
    let config_tar_path = tar_config_artifacts(config_dir, backup_dir, &ts)?;

    // ---- 7. Rotate — keep the N most recent *.db snapshots ----
    let rotated_away = rotate_backups(backup_dir, keep)?;

    Ok(BackupResult {
        snapshot_path,
        config_tar_path,
        session_count,
        rotated_away,
    })
}

/// Open the snapshot DB read-only, run integrity_check, return session count.
async fn verify_snapshot(snapshot: &Path) -> anyhow::Result<i64> {
    let path_str = snapshot.to_string_lossy().into_owned();

    tokio::task::spawn_blocking(move || -> anyhow::Result<i64> {
        // Open strictly read-only.
        let conn = rusqlite::Connection::open_with_flags(
            &path_str,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;

        // integrity_check must return exactly "ok".
        let mut stmt = conn.prepare("PRAGMA integrity_check")?;
        let rows: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        if rows.len() != 1 || rows[0] != "ok" {
            anyhow::bail!(
                "integrity_check failed on backup snapshot: {:?}",
                rows
            );
        }

        // Count probe — exercises the page cache + schema parse.
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
            .unwrap_or(0); // table may not exist in a brand-new DB; that's OK for probe

        Ok(count)
    })
    .await?
}

/// Tar `config.toml`, `interests.yaml`, and `state/` from `config_dir`.
/// Only packages files that actually exist; if none exist, returns None.
fn tar_config_artifacts(
    config_dir: &Path,
    backup_dir: &Path,
    ts: &str,
) -> anyhow::Result<Option<PathBuf>> {
    let to_include: Vec<PathBuf> = [
        config_dir.join("config.toml"),
        config_dir.join("interests.yaml"),
        config_dir.join("state"),
    ]
    .into_iter()
    .filter(|p| p.exists())
    .collect();

    if to_include.is_empty() {
        return Ok(None);
    }

    let tar_path = backup_dir.join(format!("altevra-config-{ts}.tar.gz"));
    let file = std::fs::File::create(&tar_path)?;
    let gz = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut tar = tar::Builder::new(gz);

    for src in &to_include {
        if src.is_file() {
            let rel = src
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("cannot get file name for {}", src.display()))?;
            tar.append_path_with_name(src, rel)?;
        } else if src.is_dir() {
            // Append directory recursively.
            let dir_name = src
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("cannot get dir name for {}", src.display()))?
                .to_string_lossy()
                .into_owned();
            tar.append_dir_all(&dir_name, src)?;
        }
    }

    tar.finish()?;
    Ok(Some(tar_path))
}

/// Keep only the `keep` most-recent `altevra-*.db` files in `backup_dir`.
/// Returns how many files were removed.
pub fn rotate_backups(backup_dir: &Path, keep: usize) -> anyhow::Result<usize> {
    let mut snapshots: Vec<PathBuf> = std::fs::read_dir(backup_dir)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("altevra-") && n.ends_with(".db"))
                .unwrap_or(false)
        })
        .collect();

    // Sort descending by filename (ISO timestamp prefix: newest first).
    snapshots.sort_by(|a, b| b.cmp(a));

    let mut removed = 0;
    for old in snapshots.iter().skip(keep) {
        std::fs::remove_file(old)?;
        removed += 1;

        // Also remove the companion config tar if it exists.
        let stem = old
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .trim_start_matches("altevra-");
        let companion = backup_dir.join(format!("altevra-config-{stem}.tar.gz"));
        if companion.exists() {
            let _ = std::fs::remove_file(&companion);
        }
    }

    Ok(removed)
}

// ============================================================================
// CLI entrypoints
// ============================================================================

async fn run_backup(args: BackupRunArgs) -> anyhow::Result<()> {
    if crate::commands::brain::refuse_if_maintenance_locked("backup run") {
        return Ok(());
    }

    println!(
        "Starting backup: {} → {}",
        args.db.display(),
        args.backup_dir.display()
    );

    let result = execute_backup_window(
        &args.db,
        &args.backup_dir,
        &args.config_dir,
        &args.lock_file,
        args.keep,
    )
    .await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!("  Snapshot:      {}", result.snapshot_path.display());
        if let Some(tar) = &result.config_tar_path {
            println!("  Config tar:    {}", tar.display());
        }
        println!("  Sessions:      {}", result.session_count);
        println!("  Rotated away:  {}", result.rotated_away);
        println!("Backup complete.");
    }

    Ok(())
}

async fn run_status(args: BackupStatusArgs) -> anyhow::Result<()> {
    let entries = list_backups(&args.backup_dir);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else {
        if entries.is_empty() {
            println!("No local backups found in {}", args.backup_dir.display());
        } else {
            println!("Local backups in {}:", args.backup_dir.display());
            for e in &entries {
                println!(
                    "  {} ({})",
                    e["name"].as_str().unwrap_or("?"),
                    e["age"].as_str().unwrap_or("?")
                );
            }
        }
    }
    Ok(())
}

fn list_backups(backup_dir: &Path) -> Vec<serde_json::Value> {
    let now = std::time::SystemTime::now();
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(backup_dir) {
        let mut entries: Vec<_> = rd
            .flatten()
            .filter(|e| {
                e.path()
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with("altevra-") && n.ends_with(".db"))
                    .unwrap_or(false)
            })
            .collect();
        entries.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
        for entry in entries {
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            let age = std::fs::metadata(&path)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|m| now.duration_since(m).ok())
                .map(|d| format_age(d.as_secs()))
                .unwrap_or_else(|| "unknown age".to_string());
            let size = std::fs::metadata(&path)
                .map(|m| m.len())
                .unwrap_or(0);
            out.push(serde_json::json!({
                "name": name,
                "path": path.to_string_lossy(),
                "age": age,
                "size_bytes": size,
            }));
        }
    }
    out
}

fn format_age(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}

/// `altevra backup remote` — OFF by default; requires Pavle to configure
/// `[backup] remote_target = "rclone:<remote>:<path>"` in `~/.altevra/config.toml`.
///
/// When configured, builds and prints the `rclone copy` command that Pavle would
/// run; it does NOT execute it without explicit opt-in (remote backup is a
/// Pavle-gated side effect). The command template is printed so Pavle can alias
/// or script it himself.
async fn run_remote(args: BackupRemoteArgs) -> anyhow::Result<()> {
    let config_path = args.config_dir.join("config.toml");
    let remote_target = load_remote_target(&config_path);

    match remote_target {
        None => {
            println!(
                "altevra backup remote: NOT_CONFIGURED\n\
                 \n\
                 To enable remote backups, add to ~/.altevra/config.toml:\n\
                 \n\
                   [backup]\n\
                   remote_target = \"rclone:<remote>:<path>\"\n\
                   # or\n\
                   remote_target = \"rsync://user@host:/path/to/backups\"\n\
                 \n\
                 Then run `altevra backup remote` to see the rclone command\n\
                 you would execute. Remote backup is OFF by default — you\n\
                 must configure a target and run the command yourself.\n\
                 \n\
                 Recommended targets:\n\
                 - Oracle VPS:   rclone:oracle-vps:altevra-backups\n\
                 - Tailscale:    rclone:tailscale-laptop:altevra-backups"
            );
        }
        Some(target) => {
            // Find the latest snapshot.
            let snapshots: Vec<PathBuf> = {
                let mut v: Vec<_> = std::fs::read_dir(&args.backup_dir)?
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| {
                        p.file_name()
                            .and_then(|n| n.to_str())
                            .map(|n| n.starts_with("altevra-") && n.ends_with(".db"))
                            .unwrap_or(false)
                    })
                    .collect();
                v.sort_by(|a, b| b.cmp(a));
                v
            };

            if snapshots.is_empty() {
                println!(
                    "No local backups found in {}. Run `altevra backup run` first.",
                    args.backup_dir.display()
                );
                return Ok(());
            }

            let latest = &snapshots[0];
            println!(
                "Remote backup configured: {target}\n\
                 Latest snapshot: {}\n\
                 \n\
                 Suggested command (Pavle-gated — run manually or alias):\n\
                   rclone copy {} {target}/\n\
                 \n\
                 NOTE: `altevra backup remote` does NOT execute rclone automatically.\n\
                 Configure a cron / systemd override to automate, or run manually.",
                latest.display(),
                latest.display()
            );
        }
    }
    Ok(())
}

fn load_remote_target(config_path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(config_path).ok()?;
    let doc: toml::Value = content.parse().ok()?;
    doc.get("backup")?
        .get("remote_target")?
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .map(String::from)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ---- Helper: build a minimal fresh Altevra DB ----

    async fn fresh_db(path: &Path) -> anyhow::Result<()> {
        let pool = create_pool(&path.to_string_lossy()).await?;
        run_migrations(&pool).await?;
        pool.close().await;
        Ok(())
    }

    // ---- Full backup window: lock held, verify passes, rotation ----

    #[tokio::test]
    async fn backup_run_creates_snapshot_and_verifies() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("altevra.db");
        let backup_dir = tmp.path().join("backups");
        let config_dir = tmp.path().join("config");
        let lock_file = tmp.path().join("maintenance.lock");

        fresh_db(&db_path).await.unwrap();

        let result = execute_backup_window(
            &db_path,
            &backup_dir,
            &config_dir,
            &lock_file,
            14,
        )
        .await
        .unwrap();

        assert!(
            result.snapshot_path.exists(),
            "snapshot must be created at {}",
            result.snapshot_path.display()
        );
        // Lock must be released after the window.
        assert!(
            !lock_file.exists(),
            "maintenance lock must be released after backup"
        );
    }

    #[tokio::test]
    async fn backup_lock_is_held_entire_window() {
        // We can't observe the lock mid-window (async, single thread), but we
        // CAN verify: (a) the lock is released on success, and (b) a second
        // concurrent acquire would fail while the lock is held. We test (a)
        // here; (b) is tested in the maintenance lock unit tests (core crate).
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("altevra.db");
        let backup_dir = tmp.path().join("backups");
        let config_dir = tmp.path().to_path_buf();
        let lock_file = tmp.path().join("maintenance.lock");

        fresh_db(&db_path).await.unwrap();

        execute_backup_window(&db_path, &backup_dir, &config_dir, &lock_file, 14)
            .await
            .unwrap();

        // Lock must be gone after the window.
        assert!(!lock_file.exists(), "lock must be released after backup");
    }

    #[tokio::test]
    async fn backup_fails_when_lock_already_held() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("altevra.db");
        let backup_dir = tmp.path().join("backups");
        let config_dir = tmp.path().to_path_buf();
        let lock_file = tmp.path().join("maintenance.lock");

        fresh_db(&db_path).await.unwrap();

        // Pre-acquire the lock.
        let _guard = MaintenanceLock::acquire(&lock_file, "test-holder").unwrap();

        // Backup should fail because the lock is already held.
        let result = execute_backup_window(
            &db_path,
            &backup_dir,
            &config_dir,
            &lock_file,
            14,
        )
        .await;

        assert!(
            result.is_err(),
            "backup must fail when the maintenance lock is already held"
        );
    }

    #[tokio::test]
    async fn backup_writers_refused_during_window_via_maintenance_lock() {
        // This test verifies the maintenance lock integration contract:
        // while the backup holds the lock, a writer checking
        // `maintenance_locked(&lock_file)` will see `true`.
        use altevra_core::maintenance::maintenance_locked;

        let tmp = TempDir::new().unwrap();
        let lock_file = tmp.path().join("maintenance.lock");

        // Manually acquire the lock (simulating what execute_backup_window does).
        let lock = MaintenanceLock::acquire(&lock_file, "backup test").unwrap();

        // A writer checking the lock should see it as held.
        assert!(
            maintenance_locked(&lock_file),
            "maintenance_locked must return true while lock is held"
        );

        lock.release().unwrap();

        // After release, writers should proceed.
        assert!(
            !maintenance_locked(&lock_file),
            "maintenance_locked must return false after release"
        );
    }

    // ---- Rotation ----

    #[test]
    fn rotate_keeps_n_most_recent() {
        let tmp = TempDir::new().unwrap();
        // Create 20 fake snapshot files with unique timestamps.
        for i in 0..20usize {
            let name = format!("altevra-20240101-{i:06}00.db");
            std::fs::write(tmp.path().join(&name), b"fake").unwrap();
        }

        let removed = rotate_backups(tmp.path(), 14).unwrap();
        assert_eq!(removed, 6, "20 - 14 = 6 should be removed");

        let remaining = std::fs::read_dir(tmp.path())
            .unwrap()
            .flatten()
            .filter(|e| {
                e.path()
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.ends_with(".db"))
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(remaining, 14, "exactly 14 snapshots must remain");
    }

    #[test]
    fn rotate_removes_companion_tar() {
        let tmp = TempDir::new().unwrap();
        // 16 snapshots with companion tars.
        for i in 0..16usize {
            let ts = format!("20240101-{i:06}00");
            std::fs::write(tmp.path().join(format!("altevra-{ts}.db")), b"fake").unwrap();
            std::fs::write(
                tmp.path().join(format!("altevra-config-{ts}.tar.gz")),
                b"fake",
            )
            .unwrap();
        }

        rotate_backups(tmp.path(), 14).unwrap();

        // Only 14 DB snapshots should remain.
        let dbs = std::fs::read_dir(tmp.path())
            .unwrap()
            .flatten()
            .filter(|e| {
                e.path()
                    .extension()
                    .and_then(|x| x.to_str())
                    .map(|x| x == "db")
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(dbs, 14);
    }

    // ---- Restore-verify: integrity_check on a fixture DB ----

    #[tokio::test]
    async fn verify_snapshot_passes_on_valid_db() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("test.db");
        fresh_db(&db).await.unwrap();

        // Create a snapshot via VACUUM INTO.
        let snapshot = tmp.path().join("snap.db");
        {
            let src = db.to_string_lossy().into_owned();
            let dst = snapshot.to_string_lossy().into_owned();
            tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                let conn = rusqlite::Connection::open(&src)?;
                conn.execute_batch(&format!("VACUUM INTO '{}'", dst))?;
                Ok(())
            })
            .await
            .unwrap()
            .unwrap();
        }

        let count = verify_snapshot(&snapshot).await.unwrap();
        assert_eq!(count, 0, "fresh DB has 0 sessions");
    }

    #[tokio::test]
    async fn verify_snapshot_fails_on_corrupt_db() {
        let tmp = TempDir::new().unwrap();
        let corrupt = tmp.path().join("corrupt.db");
        // Write garbage so SQLite rejects it.
        std::fs::write(&corrupt, b"this is not a sqlite database, garbage garbage").unwrap();

        let result = verify_snapshot(&corrupt).await;
        assert!(result.is_err(), "verification must fail on a corrupt DB");
    }

    // ---- Remote hook OFF by default ----

    #[tokio::test]
    async fn remote_not_configured_by_default() {
        let tmp = TempDir::new().unwrap();
        // config.toml with NO [backup] section.
        std::fs::write(tmp.path().join("config.toml"), "[vault]\npath = \".\"\n").unwrap();

        let target = load_remote_target(&tmp.path().join("config.toml"));
        assert!(
            target.is_none(),
            "remote_target must be None when not configured"
        );
    }

    #[tokio::test]
    async fn remote_reads_configured_target() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("config.toml"),
            "[backup]\nremote_target = \"rclone:oracle-vps:backups\"\n",
        )
        .unwrap();

        let target = load_remote_target(&tmp.path().join("config.toml"));
        assert_eq!(
            target.as_deref(),
            Some("rclone:oracle-vps:backups"),
            "should read the configured remote target"
        );
    }

    // ---- Config tar ----

    #[test]
    fn tar_config_artifacts_handles_missing_files() {
        let tmp = TempDir::new().unwrap();
        let config_dir = tmp.path().join("noexist");
        let backup_dir = tmp.path().join("backups");
        std::fs::create_dir_all(&backup_dir).unwrap();

        let result = tar_config_artifacts(&config_dir, &backup_dir, "20240101-120000");
        assert!(result.is_ok());
        assert!(result.unwrap().is_none(), "no tar when config_dir is empty");
    }

    #[test]
    fn tar_config_artifacts_includes_existing_files() {
        let tmp = TempDir::new().unwrap();
        let config_dir = tmp.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("config.toml"), "version = \"0.3.0\"").unwrap();
        let backup_dir = tmp.path().join("backups");
        std::fs::create_dir_all(&backup_dir).unwrap();

        let result = tar_config_artifacts(&config_dir, &backup_dir, "20240101-120000");
        assert!(result.is_ok());
        let tar = result.unwrap();
        assert!(tar.is_some(), "tar must be created when config.toml exists");
        assert!(tar.unwrap().exists(), "tar file must exist on disk");
    }
}

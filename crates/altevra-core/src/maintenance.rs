//! Maintenance lock + hook spool locations (P0 — `altevra db unify`).
//!
//! While `db unify` rewrites the canonical database, every other writer must
//! stand down. The contract (PLAN-ALIVE §P0.2):
//!
//! * **Batch writers** (brain, embedder, import, MCP write paths) check
//!   [`maintenance_locked`] and **refuse non-fatally** (print + exit 0).
//! * **Hooks** must never block the host tool, so `hook_handle` spools the
//!   event to [`spool_dir`] (one file per event, `O_EXCL`, mode `0600`,
//!   payload guard-redacted *before* disk) and exits 0. `altevra db
//!   replay-spool` drains the spool after unify via direct-by-id ingest.
//!
//! The lock carries a **stale-lock TTL** ([`MAINTENANCE_LOCK_TTL_SECS`]): a
//! crashed unify can never deadlock the recorder forever — once the lock file
//! is older than the TTL it is treated as stale (ignored by checkers, removed
//! by the next acquirer).

use std::path::{Path, PathBuf};

use crate::paths::home_dir;

/// How long a maintenance lock is honored before being considered stale.
/// A real unify on Pavle's data is seconds-to-minutes; 30 minutes is a
/// generous ceiling that still guarantees the recorder self-heals after a
/// crashed unify.
pub const MAINTENANCE_LOCK_TTL_SECS: u64 = 30 * 60;

/// Canonical absolute lock path: `$HOME/.altevra/state/maintenance.lock`.
pub fn maintenance_lock_path() -> PathBuf {
    home_dir().join(".altevra/state/maintenance.lock")
}

/// Canonical absolute spool directory: `$HOME/.altevra/state/spool`.
/// $HOME-anchored, never CWD (a hook can fire from any directory).
pub fn spool_dir() -> PathBuf {
    home_dir().join(".altevra/state/spool")
}

/// Is a (non-stale) maintenance lock held at `path`?
///
/// Staleness is judged by file mtime against [`MAINTENANCE_LOCK_TTL_SECS`]:
/// a lock older than the TTL is ignored (the unify that wrote it is presumed
/// dead). A lock file we cannot stat/parse is treated as **held** until the
/// TTL elapses (fail-safe for writers, self-healing for the recorder).
pub fn maintenance_locked(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false; // no lock file
    };
    match meta.modified().and_then(|m| {
        m.elapsed()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    }) {
        Ok(age) => age.as_secs() < MAINTENANCE_LOCK_TTL_SECS,
        // Clock skew / unreadable mtime: treat as held (fail-safe).
        Err(_) => true,
    }
}

/// Convenience: [`maintenance_locked`] at the canonical path.
pub fn maintenance_locked_default() -> bool {
    maintenance_locked(&maintenance_lock_path())
}

/// RAII maintenance lock. Acquired with `O_EXCL` (atomic create); a stale
/// lock (older than TTL) is removed and re-acquired. Released explicitly via
/// [`MaintenanceLock::release`] or best-effort on drop.
pub struct MaintenanceLock {
    path: PathBuf,
    released: bool,
}

impl MaintenanceLock {
    /// Acquire the lock at `path`. Fails if a live (non-stale) lock exists.
    pub fn acquire(path: &Path, reason: &str) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        for attempt in 0..2 {
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            match opts.open(path) {
                Ok(mut f) => {
                    use std::io::Write;
                    let body = format!(
                        "pid={}\nreason={}\nacquired_unix={}\n",
                        std::process::id(),
                        reason,
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0),
                    );
                    f.write_all(body.as_bytes())?;
                    return Ok(Self {
                        path: path.to_path_buf(),
                        released: false,
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if attempt == 0 && !maintenance_locked(path) {
                        // Stale lock — remove and retry the atomic create.
                        let _ = std::fs::remove_file(path);
                        continue;
                    }
                    anyhow::bail!(
                        "maintenance lock already held at {} (another unify in \
                         progress?); stale locks expire after {}s",
                        path.display(),
                        MAINTENANCE_LOCK_TTL_SECS
                    );
                }
                Err(e) => return Err(e.into()),
            }
        }
        anyhow::bail!(
            "could not acquire maintenance lock at {}",
            path.display()
        );
    }

    /// Explicitly release the lock (preferred over relying on Drop).
    pub fn release(mut self) -> anyhow::Result<()> {
        self.released = true;
        std::fs::remove_file(&self.path)?;
        Ok(())
    }
}

impl Drop for MaintenanceLock {
    fn drop(&mut self) {
        if !self.released {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn lock_acquire_release_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("state/maintenance.lock");
        assert!(!maintenance_locked(&path), "no file → not locked");

        let lock = MaintenanceLock::acquire(&path, "test").unwrap();
        assert!(maintenance_locked(&path), "fresh lock is held");

        // Second acquire fails while the first is live.
        assert!(MaintenanceLock::acquire(&path, "test2").is_err());

        lock.release().unwrap();
        assert!(!maintenance_locked(&path), "released → not locked");
    }

    #[test]
    fn stale_lock_is_ignored_and_reacquirable() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("maintenance.lock");
        std::fs::write(&path, "pid=1\n").unwrap();
        // Age the file past the TTL.
        let old = std::time::SystemTime::now()
            - std::time::Duration::from_secs(MAINTENANCE_LOCK_TTL_SECS + 60);
        let ft = filetime::FileTime::from_system_time(old);
        filetime::set_file_mtime(&path, ft).unwrap();

        assert!(
            !maintenance_locked(&path),
            "lock older than TTL is stale → not locked"
        );
        // A new acquirer steals the stale lock.
        let lock = MaintenanceLock::acquire(&path, "steal").unwrap();
        assert!(maintenance_locked(&path));
        lock.release().unwrap();
    }

    #[test]
    fn drop_releases_lock() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("maintenance.lock");
        {
            let _lock = MaintenanceLock::acquire(&path, "scoped").unwrap();
            assert!(maintenance_locked(&path));
        }
        assert!(!maintenance_locked(&path), "drop removes the lock file");
    }

    #[cfg(unix)]
    #[test]
    fn lock_file_mode_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("maintenance.lock");
        let lock = MaintenanceLock::acquire(&path, "perm").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "lock file must be 0600");
        lock.release().unwrap();
    }

    #[test]
    fn default_paths_are_home_anchored() {
        let h = home_dir();
        assert!(maintenance_lock_path().starts_with(&h));
        assert!(spool_dir().starts_with(&h));
        assert!(spool_dir().is_absolute());
    }
}

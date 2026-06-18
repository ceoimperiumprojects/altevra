//! Watcher daemon. Subscribes to filesystem events on configured directories,
//! debounces, hashes changed files, and emits records into:
//!
//!   * `.altevra/events/file_changes.jsonl` (append-only stream)
//!   * `pending_indexing` SQLite table (queue for the embedder worker)
//!
//! Designed to be CPU-cheap: notify provides the kernel-level inotify (Linux)
//! / FSEvents (macOS) bridge; we only act on events that pass the ignore
//! filter and survive the debounce window.

use chrono::Utc;
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::watch;
use uuid::Uuid;

use crate::debouncer::Debouncer;
use crate::hasher::short_hash;

const DEFAULT_DEBOUNCE_MS: u64 = 1000;
const DEFAULT_IGNORES: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    ".altevra",
    ".obsidian",
    ".trash",
    "__pycache__",
    ".venv",
    "dist",
    "build",
    ".next",
    ".cache",
    ".turbo",
    ".svelte-kit",
    ".parcel-cache",
    ".pytest_cache",
    "venv",
];

#[derive(Debug, Clone)]
pub struct WatcherConfig {
    pub vault_paths: Vec<PathBuf>,
    pub repo_paths: Vec<PathBuf>,
    pub debounce_ms: u64,
    pub index_code_files: bool,
    pub event_log_path: PathBuf,
    pub db_path: Option<PathBuf>,
    pub ignore_patterns: Vec<String>,
    /// File extensions (without dot) that always queue for indexing.
    pub primary_extensions: Vec<String>,
    /// Extensions queued only when `index_code_files = true`.
    pub code_extensions: Vec<String>,
}

impl Default for WatcherConfig {
    fn default() -> Self {
        Self {
            vault_paths: vec![],
            repo_paths: vec![],
            debounce_ms: DEFAULT_DEBOUNCE_MS,
            index_code_files: false,
            event_log_path: altevra_core::home_dir()
                .join(".altevra/events/file_changes.jsonl"),
            db_path: None,
            ignore_patterns: DEFAULT_IGNORES.iter().map(|s| s.to_string()).collect(),
            primary_extensions: vec!["md".into(), "mdc".into(), "yaml".into(), "yml".into()],
            code_extensions: vec![
                "rs".into(),
                "ts".into(),
                "tsx".into(),
                "py".into(),
                "js".into(),
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChangeRecord {
    pub id: Uuid,
    pub path: String,
    pub kind: String,
    pub before_hash: Option<String>,
    pub after_hash: Option<String>,
    pub ts: chrono::DateTime<chrono::Utc>,
}

pub struct WatcherDaemon {
    config: WatcherConfig,
}

impl WatcherDaemon {
    pub fn new(config: WatcherConfig) -> Self {
        Self { config }
    }

    /// Run forever until `shutdown_rx` flips to true.
    pub async fn run(self, mut shutdown_rx: watch::Receiver<bool>) -> anyhow::Result<()> {
        let (tx, mut rx) = mpsc::unbounded_channel::<notify::Event>();
        let tx_clone = tx.clone();
        let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |res| {
            if let Ok(evt) = res {
                let _ = tx_clone.send(evt);
            }
        })?;
        watcher.configure(Config::default().with_poll_interval(Duration::from_secs(2)))?;

        for path in self
            .config
            .vault_paths
            .iter()
            .chain(self.config.repo_paths.iter())
        {
            if path.exists() {
                let _ = watcher.watch(path, RecursiveMode::Recursive);
            }
        }

        let mut debouncer = Debouncer::new(self.config.debounce_ms);
        let mut interval = tokio::time::interval(Duration::from_millis(200));

        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        break;
                    }
                }
                _ = interval.tick() => {
                    for path in debouncer.drain_ready() {
                        if let Err(e) = self.emit(&path, "modify").await {
                            tracing::warn!("watcher emit failed for {}: {e}", path.display());
                        }
                    }
                }
                Some(evt) = rx.recv() => {
                    for path in evt.paths {
                        if !self.should_track(&path) {
                            continue;
                        }
                        match evt.kind {
                            EventKind::Create(_) | EventKind::Modify(_) => {
                                debouncer.touch(&path);
                            }
                            EventKind::Remove(_) => {
                                let _ = self.emit(&path, "remove").await;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn should_track(&self, path: &Path) -> bool {
        // Ignore via pattern.
        for ig in &self.config.ignore_patterns {
            if path.components().any(|c| c.as_os_str() == ig.as_str()) {
                return false;
            }
        }
        // Filter by extension.
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        if self.config.primary_extensions.iter().any(|p| p == &ext) {
            return true;
        }
        if self.config.index_code_files && self.config.code_extensions.iter().any(|p| p == &ext) {
            return true;
        }
        false
    }

    async fn emit(&self, path: &Path, kind: &str) -> anyhow::Result<()> {
        let record = FileChangeRecord {
            id: Uuid::new_v4(),
            path: path.to_string_lossy().to_string(),
            kind: kind.to_string(),
            before_hash: None,
            after_hash: short_hash(path),
            ts: Utc::now(),
        };

        // Append to JSONL log.
        if let Some(parent) = self.config.event_log_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.config.event_log_path)
        {
            let _ = writeln!(f, "{}", serde_json::to_string(&record)?);
        }

        // Queue in DB if configured.
        if let Some(db) = &self.config.db_path {
            if let Ok(pool) = altevra_db::create_pool(&db.to_string_lossy()).await {
                let _ = altevra_db::run_migrations(&pool).await;
                let _ = sqlx::query(
                    r#"INSERT INTO pending_indexing (id, path, status)
                       VALUES (?, ?, 'pending')
                       ON CONFLICT (path) DO UPDATE SET
                         queued_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                         status = 'pending'"#,
                )
                .bind(Uuid::new_v4().to_string())
                .bind(&record.path)
                .execute(&pool)
                .await;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignore_patterns_skip_known_dirs() {
        let cfg = WatcherConfig::default();
        let d = WatcherDaemon { config: cfg };
        assert!(!d.should_track(Path::new("/repo/.git/HEAD")));
        assert!(!d.should_track(Path::new("/repo/target/debug/foo")));
        assert!(!d.should_track(Path::new("/repo/.altevra/state/x.md")));
        assert!(!d.should_track(Path::new("/repo/node_modules/pkg/index.js")));
    }

    #[test]
    fn md_files_always_tracked() {
        let cfg = WatcherConfig::default();
        let d = WatcherDaemon { config: cfg };
        assert!(d.should_track(Path::new("/repo/notes/idea.md")));
        assert!(d.should_track(Path::new("/repo/notes/rule.mdc")));
    }

    #[test]
    fn code_files_only_when_enabled() {
        let mut cfg = WatcherConfig::default();
        let d = WatcherDaemon {
            config: cfg.clone(),
        };
        assert!(!d.should_track(Path::new("/repo/src/main.rs")));

        cfg.index_code_files = true;
        let d2 = WatcherDaemon { config: cfg };
        assert!(d2.should_track(Path::new("/repo/src/main.rs")));
    }

    #[tokio::test]
    async fn emit_writes_jsonl_record() {
        let tmp = tempfile::TempDir::new().unwrap();
        let log = tmp.path().join("events.jsonl");
        let file_to_track = tmp.path().join("note.md");
        std::fs::write(&file_to_track, b"hello").unwrap();

        let cfg = WatcherConfig {
            event_log_path: log.clone(),
            ..WatcherConfig::default()
        };
        let d = WatcherDaemon { config: cfg };
        d.emit(&file_to_track, "modify").await.unwrap();

        let content = std::fs::read_to_string(&log).unwrap();
        assert!(content.contains("\"kind\":\"modify\""));
        assert!(content.contains("note.md"));
    }
}

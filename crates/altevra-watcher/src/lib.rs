//! Altevra v0.3.2 — File Watcher Daemon.
//!
//! Watches vault + repo directories with `notify`, debounces rapid changes,
//! emits `FileChanged` records into a JSONL event log AND queues an entry in
//! the `pending_indexing` SQLite table so the continuous embedder can pick
//! the file up for re-embedding.

pub mod daemon;
pub mod debouncer;
pub mod hasher;

pub use daemon::{WatcherConfig, WatcherDaemon};
pub use debouncer::Debouncer;
pub use hasher::short_hash;

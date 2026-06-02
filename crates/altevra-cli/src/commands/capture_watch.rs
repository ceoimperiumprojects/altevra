//! `altevra capture --watch` — auto-atomize living docs on save.
//!
//! Closes the atomization loop: instead of running `altevra capture <file>`
//! by hand, this watches a set of directories (default `~/Obsidian/Imperium/
//! Memory/` + `Daily/`) and, when a `.md` settles after an edit, re-runs the
//! EXISTING atomize logic ([`super::capture::atomize_file`]) on that file into the
//! configured DB.
//!
//! Incremental + idempotent: `atomize_file` reconciles each file's prior objects
//! (same `capture-<filestem>-` id prefix) — an edited section updates (new content
//! hash → new id, stale id forgotten), a deleted section is forgotten, a new
//! section is added. SQLite writes ONLY — the vault is never touched (SI-7 holds;
//! domain inference still escalates high-water to Restricted inside `atomize_file`).
//!
//! Async watcher mirroring `altevra-watcher::WatcherDaemon`: `notify` →
//! `tokio::sync::mpsc` → `Debouncer` → `tokio::select!`, with a `watch` shutdown
//! channel driven by Ctrl+C (so the per-cycle atomize can `.await` the DB writes —
//! unlike the synchronous skill-sync watcher).

use crate::commands::capture::atomize_file;
use altevra_core::domain::Domain;
use altevra_core::security::Sensitivity;
use altevra_db::{create_pool, run_migrations};
use altevra_vault::parse_sections;
use altevra_watcher::Debouncer;
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::{mpsc, watch};

/// Watcher configuration for capture auto-atomize.
#[derive(Debug, Clone)]
pub struct CaptureWatchConfig {
    /// Directories to watch (recursively) for living-doc `.md` changes.
    pub paths: Vec<PathBuf>,
    /// Coalesce window for editor save bursts.
    pub debounce_ms: u64,
    /// Declared sensitivity floor passed to the atomizer (guard may raise it).
    pub declared: Sensitivity,
    /// Category seeds passed through to each atomized object.
    pub categories: Vec<String>,
    /// SQLite DB to atomize into.
    pub db: PathBuf,
}

/// Default watched dirs: the living Memory aggregates + the Daily notes.
pub fn default_watch_dirs() -> Vec<PathBuf> {
    let base = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("Obsidian")
        .join("Imperium");
    vec![base.join("Memory"), base.join("Daily")]
}

/// Is this path an interesting living-doc change? `.md` only; skip our own write
/// temps and editor swap/hidden files (mirrors the skill watcher filter). NOTE:
/// the capture watcher writes ONLY to SQLite, so there is no `.altevra-tmp` of our
/// own to loop on — the filter is still applied for editor `.swp`/hidden safety.
pub fn is_relevant_capture_path(p: &Path) -> bool {
    let name = match p.file_name().and_then(|f| f.to_str()) {
        Some(n) => n,
        None => return false,
    };
    if name.ends_with(".altevra-tmp") {
        return false;
    }
    if name.starts_with('.') {
        return false; // .foo.md.swp, hidden editor files
    }
    name.ends_with(".md")
}

/// One settled file → atomize it into the DB (open pool, read, parse, atomize).
/// Returns a compact log line (or an error string) for the cycle report. Reading a
/// missing/empty/binary file is a soft skip (returns `None`), never a panic.
pub async fn atomize_one(
    pool: &SqlitePool,
    file: &Path,
    cfg: &CaptureWatchConfig,
) -> anyhow::Result<Option<String>> {
    // The file may have been removed between the event and the settle — skip.
    let raw = match std::fs::read_to_string(file) {
        Ok(c) => c,
        Err(_) => return Ok(None),
    };
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let sections = parse_sections(&raw);
    if sections.is_empty() {
        // No `## ` sections (e.g. a freeform daily with only prose) — nothing to
        // atomize. The whole-file capture path is the one-shot command's job.
        return Ok(None);
    }
    let domain: Domain = crate::commands::capture::infer_domain(file);
    // Cross-link to known people/projects (People.md + registry + mentors).
    let dict = crate::commands::entity_dict::build_dictionary(file, None);
    let res = atomize_file(
        pool,
        file,
        &sections,
        &domain,
        &cfg.declared,
        &cfg.categories,
        Some(&dict),
    )
    .await?;
    Ok(Some(format!(
        "{}: {} {}(s) captured, {} forgotten (stale), {} need-structure, {} mention edge(s)",
        file.display(),
        res.captured,
        res.kind,
        res.forgotten,
        res.needs_structure,
        res.mentions_recorded
    )))
}

/// Run the initial atomize pass over every `.md` under the watched dirs, then
/// block watching until Ctrl+C. `on_cycle` is invoked with each settled file's log
/// line (so tests/callers can observe activity without stdout coupling).
pub async fn run_watch<F: FnMut(&str)>(
    cfg: CaptureWatchConfig,
    mut on_cycle: F,
) -> anyhow::Result<()> {
    let pool = create_pool(&cfg.db.to_string_lossy()).await?;
    run_migrations(&pool).await?;

    // --- initial pass: atomize everything currently present ---
    for dir in &cfg.paths {
        if !dir.exists() {
            continue;
        }
        for entry in walkdir::WalkDir::new(dir)
            .follow_links(false)
            .into_iter()
            .flatten()
        {
            let p = entry.path();
            if entry.file_type().is_file() && is_relevant_capture_path(p) {
                if let Some(line) = atomize_one(&pool, p, &cfg).await? {
                    on_cycle(&format!("initial → {line}"));
                }
            }
        }
    }

    // --- watch loop ---
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    // Ctrl+C flips the shutdown channel.
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = shutdown_tx.send(true);
    });
    watch_until_shutdown(&pool, &cfg, shutdown_rx, &mut on_cycle).await
}

/// The core async watch loop (separated so tests can drive shutdown directly
/// without a real Ctrl+C). Mirrors `WatcherDaemon::run`.
pub async fn watch_until_shutdown<F: FnMut(&str)>(
    pool: &SqlitePool,
    cfg: &CaptureWatchConfig,
    mut shutdown_rx: watch::Receiver<bool>,
    on_cycle: &mut F,
) -> anyhow::Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<notify::Event>();
    let tx_clone = tx.clone();
    let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |res| {
        if let Ok(evt) = res {
            let _ = tx_clone.send(evt);
        }
    })?;
    watcher.configure(Config::default().with_poll_interval(Duration::from_secs(2)))?;

    let mut watched_any = false;
    for dir in &cfg.paths {
        if dir.exists() && watcher.watch(dir, RecursiveMode::Recursive).is_ok() {
            watched_any = true;
        }
    }
    if !watched_any {
        anyhow::bail!(
            "no watch directories exist (looked for {})",
            cfg.paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    let mut debouncer = Debouncer::new(cfg.debounce_ms);
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
                    match atomize_one(pool, &path, cfg).await {
                        Ok(Some(line)) => on_cycle(&format!("↻ {line}")),
                        Ok(None) => {}
                        Err(e) => on_cycle(&format!("⚠ atomize failed for {}: {e}", path.display())),
                    }
                }
            }
            Some(evt) = rx.recv() => {
                if !matches!(
                    evt.kind,
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                ) {
                    continue;
                }
                for p in evt.paths {
                    if is_relevant_capture_path(&p) {
                        debouncer.touch(&p);
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relevant_path_filter() {
        assert!(is_relevant_capture_path(Path::new(
            "/v/Memory/Decisions.md"
        )));
        assert!(is_relevant_capture_path(Path::new(
            "/v/Daily/2026-06-02.md"
        )));
        assert!(!is_relevant_capture_path(Path::new(
            "/v/Memory/Decisions.md.altevra-tmp"
        )));
        assert!(!is_relevant_capture_path(Path::new(
            "/v/Memory/.Decisions.md.swp"
        )));
        assert!(!is_relevant_capture_path(Path::new("/v/Memory/notes.txt")));
    }

    #[test]
    fn default_dirs_point_at_memory_and_daily() {
        let dirs = default_watch_dirs();
        assert!(dirs.iter().any(|d| d.ends_with("Memory")));
        assert!(dirs.iter().any(|d| d.ends_with("Daily")));
    }

    fn cfg_for(dir: &Path, db: &Path) -> CaptureWatchConfig {
        CaptureWatchConfig {
            paths: vec![dir.to_path_buf()],
            debounce_ms: 50,
            declared: "internal".parse().unwrap(),
            categories: vec![],
            db: db.to_path_buf(),
        }
    }

    #[tokio::test]
    async fn atomize_one_writes_objects_and_skips_nonexistent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mem = tmp.path().join("Memory");
        std::fs::create_dir_all(&mem).unwrap();
        let db = tmp.path().join("w.db");
        let pool = create_pool(&db.to_string_lossy()).await.unwrap();
        run_migrations(&pool).await.unwrap();
        let cfg = cfg_for(&mem, &db);

        // A real 2-section Decisions file → atomized.
        let file = mem.join("Decisions.md");
        std::fs::write(
            &file,
            "# Decisions\n\n## A decision here\nbody one.\n\n## Another decision\nbody two.\n",
        )
        .unwrap();
        let line = atomize_one(&pool, &file, &cfg).await.unwrap();
        assert!(line.is_some(), "a real file produces a cycle line");

        let cands = altevra_db::ObjectIndexRepository::new(&pool)
            .candidates(None)
            .await
            .unwrap();
        assert_eq!(cands.len(), 2, "two sections atomized");

        // A missing file is a soft skip (no panic, no line).
        let gone = mem.join("Nope.md");
        assert!(atomize_one(&pool, &gone, &cfg).await.unwrap().is_none());
    }

    /// Live watcher loop: create a file AFTER the watch starts, let the debounce
    /// settle, then signal shutdown. The object must land in the DB — proving the
    /// notify→debounce→atomize path works end-to-end (SQLite only; vault untouched).
    #[tokio::test]
    async fn watch_loop_atomizes_a_created_file_then_shuts_down() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mem = tmp.path().join("Memory");
        std::fs::create_dir_all(&mem).unwrap();
        let db = tmp.path().join("watch.db");
        let pool = create_pool(&db.to_string_lossy()).await.unwrap();
        run_migrations(&pool).await.unwrap();
        let cfg = cfg_for(&mem, &db);

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let pool_loop = pool.clone();
        let cfg_loop = cfg.clone();
        let handle = tokio::spawn(async move {
            let mut seen = 0usize;
            watch_until_shutdown(&pool_loop, &cfg_loop, shutdown_rx, &mut |_l| seen += 1)
                .await
                .unwrap();
            seen
        });

        // Give the watcher a moment to register, then create a living doc.
        tokio::time::sleep(Duration::from_millis(300)).await;
        std::fs::write(
            mem.join("Decisions.md"),
            "# Decisions\n\n## Watched decision\nthis was created while watching.\n",
        )
        .unwrap();

        // Wait long enough for the event + debounce (50ms) + a couple interval ticks.
        // Poll the DB until the object appears (or time out).
        let mut found = false;
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let n = altevra_db::ObjectIndexRepository::new(&pool)
                .candidates(None)
                .await
                .unwrap()
                .len();
            if n >= 1 {
                found = true;
                break;
            }
        }
        let _ = shutdown_tx.send(true);
        let seen = handle.await.unwrap();
        assert!(found, "the watched file's section was atomized into the DB");
        assert!(seen >= 1, "the on_cycle callback fired at least once");

        // recall reflects the live capture.
        let fts = altevra_db::FtsRepository::new(&pool);
        assert_eq!(
            fts.search_objects("created while watching", 10)
                .await
                .unwrap()
                .len(),
            1
        );
    }
}

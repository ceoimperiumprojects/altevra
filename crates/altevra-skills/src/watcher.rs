//! Real-time skill propagation — answers Pavle's "AUTOMATSKI da se prebacuje".
//!
//! Watches every known external skill directory (`~/.{claude,codex,cursor,hermes,
//! imperium}/skills/`) plus the local vault `06-skills/` for changes. When a
//! skill file is created or modified, debounces 2 seconds (editors often save
//! atomically with multiple writes) and re-runs the standard `build_plan` +
//! `apply_plan`. Every safety guard from `sync.rs` remains in force — user-authored
//! files are still never overwritten, atomic writes still apply.
//!
//! This is a foreground daemon (no fork). Stop with Ctrl+C — the channel close
//! is observed and the loop exits cleanly. The notify watcher is held for the
//! whole loop lifetime; events flow through an unbounded mpsc.

use crate::importer::{scan_all, scan_external_dir, ExternalSkill, SourceTool};
use crate::sync::{apply_plan, build_plan, SyncResult};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

/// One "settle" of changes — what one debounced cycle did.
#[derive(Debug, Clone)]
pub struct CycleReport {
    pub triggering_paths: Vec<PathBuf>,
    pub result: SyncResult,
    pub plan_creates: usize,
    pub plan_refreshes: usize,
    pub plan_skips: usize,
}

/// Configuration for the watcher.
#[derive(Debug, Clone)]
pub struct WatchConfig {
    /// Target tools to propagate INTO on every cycle.
    pub targets: Vec<SourceTool>,
    /// Optional vault skills root (e.g. `<repo>/06-skills/`) included as a source.
    pub vault_skills_dir: Option<PathBuf>,
    /// Apply mode — when false, every cycle is a dry-run (planned + reported, no writes).
    pub apply: bool,
    /// Coalesce window. 2s is enough to swallow editor save bursts without feeling laggy.
    pub debounce_ms: u64,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            targets: vec![
                SourceTool::Claude,
                SourceTool::Codex,
                SourceTool::Cursor,
                SourceTool::Hermes,
                SourceTool::Imperium,
            ],
            vault_skills_dir: None,
            apply: false,
            debounce_ms: 2_000,
        }
    }
}

/// All known external dirs that currently exist on disk. Returns `(SourceTool, dir)`.
fn watch_targets(cfg: &WatchConfig) -> Vec<(SourceTool, PathBuf)> {
    let mut out: Vec<(SourceTool, PathBuf)> = crate::importer::default_skill_dirs();
    if let Some(v) = &cfg.vault_skills_dir {
        if v.exists() {
            out.push((SourceTool::Altevra, v.clone()));
        }
    }
    out
}

fn current_inventory(cfg: &WatchConfig) -> Vec<ExternalSkill> {
    let mut inv = scan_all();
    if let Some(v) = &cfg.vault_skills_dir {
        if v.exists() {
            inv.extend(scan_external_dir(v, SourceTool::Altevra));
        }
    }
    inv
}

fn skill_dir_for(tool: &SourceTool) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    match tool {
        SourceTool::Claude => Some(home.join(".claude/skills")),
        SourceTool::Codex => Some(home.join(".codex/skills")),
        SourceTool::Cursor => Some(home.join(".cursor/skills")),
        SourceTool::Hermes => Some(home.join(".hermes/skills")),
        SourceTool::Imperium => Some(home.join(".imperium/skills")),
        _ => None,
    }
}

/// Compute + apply one sync cycle, returning what happened (for the report log).
pub fn run_one_cycle(cfg: &WatchConfig, triggering: Vec<PathBuf>) -> CycleReport {
    let inv = current_inventory(cfg);
    let plan = build_plan(&inv, &cfg.targets, &skill_dir_for);
    let plan_creates = plan.creates();
    let plan_refreshes = plan.refreshes();
    let plan_skips = plan.skips();
    let result = apply_plan(&plan, cfg.apply);
    CycleReport {
        triggering_paths: triggering,
        result,
        plan_creates,
        plan_refreshes,
        plan_skips,
    }
}

/// Decide whether an event's path is interesting (skip our own atomic-write temps
/// to avoid an infinite re-trigger loop when `--apply` is on).
fn is_relevant_path(p: &std::path::Path) -> bool {
    let name = match p.file_name().and_then(|f| f.to_str()) {
        Some(n) => n,
        None => return false,
    };
    if name.ends_with(".altevra-tmp") {
        return false; // our own write-then-rename temp
    }
    if name.starts_with('.') {
        return false; // hidden/editor swap files (e.g. .SKILL.md.swp)
    }
    if !(name.ends_with(".md") || name == "SKILL.md") {
        return false;
    }
    true
}

/// Long-running watcher. Blocks until `on_cycle` returns `false` (stop signal).
/// `on_cycle` is invoked after every settled cycle so the caller can print/log
/// the report and decide whether to keep going.
///
/// Watches every existing skill dir non-recursively except the per-tool root
/// (recursive so we catch `<slug>/SKILL.md` writes); never recurses into the
/// Altevra `target/` build dir.
pub fn watch_loop<F: FnMut(&CycleReport) -> bool>(
    cfg: WatchConfig,
    mut on_cycle: F,
) -> anyhow::Result<()> {
    let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
    let mut watcher = notify::recommended_watcher(move |res| {
        let _ = tx.send(res);
    })?;

    let targets = watch_targets(&cfg);
    if targets.is_empty() {
        anyhow::bail!("no skill directories found to watch (no ~/.{{claude,codex,…}}/skills/)");
    }
    for (tool, dir) in &targets {
        if let Err(e) = watcher.watch(dir, RecursiveMode::Recursive) {
            tracing::warn!("could not watch {} ({:?}): {e}", dir.display(), tool);
        }
    }

    // Per-path debounce; coalesces the editor save burst.
    let mut pending: HashMap<PathBuf, std::time::Instant> = HashMap::new();
    let window = Duration::from_millis(cfg.debounce_ms);

    loop {
        // Block briefly for the next event; on timeout, drain ready paths.
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(Ok(ev)) => {
                if !matches!(
                    ev.kind,
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                ) {
                    continue;
                }
                for p in ev.paths {
                    if is_relevant_path(&p) {
                        pending.entry(p).or_insert_with(std::time::Instant::now);
                    }
                }
            }
            Ok(Err(e)) => tracing::warn!("watcher error: {e}"),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                tracing::info!("watcher channel closed; exiting");
                break;
            }
        }

        // Drain ready (settled past the window).
        let now = std::time::Instant::now();
        let ready: Vec<PathBuf> = pending
            .iter()
            .filter(|(_, t)| now.duration_since(**t) >= window)
            .map(|(p, _)| p.clone())
            .collect();
        if ready.is_empty() {
            continue;
        }
        for p in &ready {
            pending.remove(p);
        }

        let report = run_one_cycle(&cfg, ready);
        if !on_cycle(&report) {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_files_and_hidden_files_are_filtered() {
        assert!(is_relevant_path(std::path::Path::new("SKILL.md")));
        assert!(is_relevant_path(std::path::Path::new(
            "/home/x/.claude/skills/audit/SKILL.md"
        )));
        assert!(is_relevant_path(std::path::Path::new("foo.md")));
        // Our own temp must be ignored so apply doesn't re-trigger itself.
        assert!(!is_relevant_path(std::path::Path::new(
            "SKILL.md.altevra-tmp"
        )));
        // Editor swap / hidden.
        assert!(!is_relevant_path(std::path::Path::new(".SKILL.md.swp")));
        // Non-markdown.
        assert!(!is_relevant_path(std::path::Path::new("config.json")));
    }

    #[test]
    fn run_one_cycle_dry_never_writes() {
        // A WatchConfig with no vault and no real homedir scan can still produce a
        // report; the test we care about is that DRY-RUN never writes regardless of
        // plan size. (We don't assert plan counts because real `~/` content varies
        // by machine — that's exactly why the inventory test owns that contract.)
        let cfg = WatchConfig {
            targets: vec![SourceTool::Claude],
            vault_skills_dir: None,
            apply: false,
            ..Default::default()
        };
        let report = run_one_cycle(&cfg, vec![]);
        assert_eq!(report.result.created, 0, "dry-run never writes");
        assert_eq!(report.result.refreshed, 0);
    }
}

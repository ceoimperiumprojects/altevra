//! Auto-detect tool storage paths + Obsidian vault locations.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct DiscoveryReport {
    pub claude_code_files: Vec<PathBuf>,
    pub codex_state: Option<PathBuf>,
    pub codex_history: Option<PathBuf>,
    /// Reserved for v0.5+ — tool call telemetry from logs_2.sqlite.
    #[allow(dead_code)]
    pub codex_logs: Option<PathBuf>,
    pub cursor_jsonl_files: Vec<PathBuf>,
    pub antigravity_history: Option<PathBuf>,
    pub hermes_session_files: Vec<PathBuf>,
    pub obsidian_vaults: Vec<PathBuf>,
}

impl DiscoveryReport {
    pub fn total_session_files(&self) -> usize {
        self.claude_code_files.len()
            + self.codex_state.iter().count()
            + self.cursor_jsonl_files.len()
            + self.antigravity_history.iter().count()
            + self.hermes_session_files.len()
    }
}

fn home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

/// Recursively glob `.jsonl` files under `root`. Bounded depth so we don't
/// wander the whole filesystem.
fn glob_jsonl(root: &Path, max_depth: usize) -> Vec<PathBuf> {
    if !root.exists() {
        return vec![];
    }
    walkdir::WalkDir::new(root)
        .max_depth(max_depth)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_type().is_file()
                && e.path().extension().and_then(|s| s.to_str()) == Some("jsonl")
        })
        .map(|e| e.path().to_path_buf())
        .collect()
}

fn first_existing(paths: &[PathBuf]) -> Option<PathBuf> {
    paths.iter().find(|p| p.exists()).cloned()
}

pub fn discover() -> DiscoveryReport {
    let h = home();
    DiscoveryReport {
        claude_code_files: glob_jsonl(&h.join(".claude/projects"), 3),
        codex_state: first_existing(&[h.join(".codex/state_5.sqlite")]),
        codex_history: first_existing(&[h.join(".codex/history.jsonl")]),
        codex_logs: first_existing(&[h.join(".codex/logs_2.sqlite")]),
        cursor_jsonl_files: discover_cursor_jsonls(&h),
        antigravity_history: first_existing(&[h.join(".gemini/antigravity-cli/history.jsonl")]),
        hermes_session_files: discover_hermes(&h),
        obsidian_vaults: discover_obsidian_vaults(&h),
    }
}

fn discover_cursor_jsonls(home: &Path) -> Vec<PathBuf> {
    // VS Code (where Cursor agent stores chat) workspace storage.
    let ws_root = home.join(".config/Code/User/workspaceStorage");
    let mut out = Vec::new();
    if ws_root.exists() {
        for entry in walkdir::WalkDir::new(&ws_root)
            .max_depth(3)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let p = entry.path();
            if p.is_file()
                && p.extension().and_then(|s| s.to_str()) == Some("jsonl")
                && p.parent()
                    .and_then(|p| p.file_name())
                    .map(|n| n == "chatSessions")
                    .unwrap_or(false)
            {
                out.push(p.to_path_buf());
            }
        }
    }
    // Cursor IDE proper (not detected on Pavle's box but supported generically).
    let cursor_root = home.join(".config/Cursor/User/workspaceStorage");
    if cursor_root.exists() {
        for entry in walkdir::WalkDir::new(&cursor_root)
            .max_depth(3)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let p = entry.path();
            if p.is_file()
                && p.extension().and_then(|s| s.to_str()) == Some("jsonl")
                && p.parent()
                    .and_then(|p| p.file_name())
                    .map(|n| n == "chatSessions")
                    .unwrap_or(false)
            {
                out.push(p.to_path_buf());
            }
        }
    }
    out
}

fn discover_hermes(home: &Path) -> Vec<PathBuf> {
    let root = home.join(".hermes/sessions");
    if !root.exists() {
        return vec![];
    }
    walkdir::WalkDir::new(&root)
        .max_depth(2)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_type().is_file()
                && e.path().extension().and_then(|s| s.to_str()) == Some("json")
                && e.path()
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with("session_"))
                    .unwrap_or(false)
        })
        .map(|e| e.path().to_path_buf())
        .collect()
}

fn discover_obsidian_vaults(home: &Path) -> Vec<PathBuf> {
    let candidates = [
        home.join("Obsidian"),
        home.join("Documents/Obsidian"),
        home.join("vaults"),
    ];
    let mut found = Vec::new();
    for root in &candidates {
        if !root.exists() {
            continue;
        }
        for entry in walkdir::WalkDir::new(root)
            .max_depth(2)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let p = entry.path();
            if p.is_dir() && p.file_name().and_then(|n| n.to_str()) == Some(".obsidian") {
                if let Some(parent) = p.parent() {
                    let pb = parent.to_path_buf();
                    if !found.contains(&pb) {
                        found.push(pb);
                    }
                }
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn empty_report_counts_zero() {
        let r = DiscoveryReport::default();
        assert_eq!(r.total_session_files(), 0);
    }

    #[test]
    fn glob_jsonl_walks_recursively() {
        let tmp = TempDir::new().unwrap();
        let sub = tmp.path().join("a/b");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("file.jsonl"), "{}\n").unwrap();
        fs::write(tmp.path().join("ignore.txt"), "x").unwrap();
        let files = glob_jsonl(tmp.path(), 5);
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("file.jsonl"));
    }

    #[test]
    fn discover_obsidian_finds_vault_via_marker() {
        let tmp = TempDir::new().unwrap();
        let vault = tmp.path().join("Obsidian/MyVault");
        fs::create_dir_all(vault.join(".obsidian")).unwrap();
        // Override HOME so discovery walks our tmp tree.
        let prev_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());
        let vaults = discover_obsidian_vaults(tmp.path());
        if let Some(h) = prev_home {
            std::env::set_var("HOME", h);
        }
        assert_eq!(vaults.len(), 1);
        assert!(vaults[0].ends_with("MyVault"));
    }
}

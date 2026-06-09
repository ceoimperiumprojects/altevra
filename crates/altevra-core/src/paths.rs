//! Canonical filesystem paths used across the Altevra workspace.
//!
//! Single source of truth for the local SQLite database location. CLI args,
//! brain scheduler, and MCP tools all import the same default so we never
//! drift between modules.
//!
//! Override via `ALTEVRA_DB_PATH` env var, useful for non-standard setups
//! (e.g. running multiple Altevra installs against separate stores).
//!
//! # Design note — `DEFAULT_DB_PATH` vs `default_db_path()`
//!
//! `DEFAULT_DB_PATH` is a bare path *suffix/name*, kept as `&'static str`
//! only for clap `default_value` attributes (which require a compile-time
//! string). It intentionally has **no leading `$HOME` component** because a
//! const cannot call `std::env::var` at compile time.
//!
//! `default_db_path()` is a *function* that computes the full absolute path
//! at runtime by anchoring the suffix under `$HOME`. All actual I/O must
//! call the function, never use the constant directly as a path.

use std::path::PathBuf;

/// Bare path suffix used only as a clap `default_value` compile-time string.
/// Do NOT open files with this value directly — call `default_db_path()`
/// instead.
pub const DEFAULT_DB_PATH: &str = ".altevra/altevra.db";

/// Bare path suffix for the PID file (clap default_value use only).
pub const DEFAULT_BRAIN_PID: &str = ".altevra/brain.pid";

/// Bare path suffix for the watcher PID file (clap default_value use only).
pub const DEFAULT_WATCHER_PID: &str = ".altevra/watcher.pid";

/// Returns the canonical absolute DB path, anchored under `$HOME/.altevra/`.
///
/// Priority:
///   1. `ALTEVRA_DB_PATH` env var (non-empty) — full override, used as-is.
///   2. `$HOME/.altevra/altevra.db` — the canonical single-DB location.
///
/// Never returns a CWD-relative path.
pub fn default_db_path() -> PathBuf {
    if let Ok(p) = std::env::var("ALTEVRA_DB_PATH") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    home_dir().join(".altevra/altevra.db")
}

/// Returns `$HOME` as a `PathBuf`. Falls back to `/tmp/altevra-fallback` on
/// systems where `HOME` is genuinely unset (sandboxes, containers). Prefer
/// this over `std::env::home_dir()` which is deprecated on some platforms.
pub fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp/altevra-fallback"))
}

/// Returns the default vault root used as the `--vault` fallback by
/// `altevra context`, `altevra memory search`, and the brain daemon.
///
/// Priority (PLAN-ALIVE P0 §3 — config-load fix):
///   1. `$HOME/.altevra/config.toml` → `[vault].path` (the shape `altevra init`
///      writes) or top-level `vault_path` (the `AltevraConfig` shape). A value
///      of `"."` is the init placeholder and is treated as unset.
///   2. `ALTEVRA_VAULT` env var (non-empty).
///   3. `"."` — the legacy CWD behavior.
///
/// A leading `~/` in the configured value expands to `$HOME`. Never panics;
/// any unreadable/unparsable config falls through to the next source.
pub fn default_vault_path() -> PathBuf {
    let cfg = home_dir().join(".altevra/config.toml");
    if let Ok(content) = std::fs::read_to_string(&cfg) {
        if let Some(p) = vault_path_from_config(&content) {
            return p;
        }
    }
    if let Ok(p) = std::env::var("ALTEVRA_VAULT") {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            return expand_home(trimmed);
        }
    }
    PathBuf::from(".")
}

/// Parse a config.toml string and extract the vault path, if meaningfully set.
/// Accepts BOTH config shapes in the wild: `[vault] path = "..."` (written by
/// `altevra init`) and top-level `vault_path = "..."` (`AltevraConfig`).
/// Returns `None` for missing, empty, or the `"."` placeholder.
fn vault_path_from_config(content: &str) -> Option<PathBuf> {
    let doc: toml::Value = content.parse().ok()?;
    let raw = doc
        .get("vault")
        .and_then(|v| v.get("path"))
        .and_then(|v| v.as_str())
        .or_else(|| doc.get("vault_path").and_then(|v| v.as_str()))?;
    let raw = raw.trim();
    if raw.is_empty() || raw == "." {
        return None;
    }
    Some(expand_home(raw))
}

/// Expand a leading `~/` to `$HOME`. Anything else passes through unchanged.
fn expand_home(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => home_dir().join(rest),
        None => PathBuf::from(p),
    }
}

/// Returns the canonical absolute brain PID file path under `$HOME/.altevra/`.
pub fn default_brain_pid_path() -> PathBuf {
    home_dir().join(".altevra/brain.pid")
}

/// Returns the canonical absolute watcher PID file path under `$HOME/.altevra/`.
pub fn default_watcher_pid_path() -> PathBuf {
    home_dir().join(".altevra/watcher.pid")
}

/// Returns the canonical absolute current-session state path, keyed by
/// `tool` and a short hash of `cwd` so concurrent sessions from different
/// tools/projects never share the same pointer file.
///
/// Format: `$HOME/.altevra/state/session-<tool>-<cwd_hash>.txt`
pub fn current_session_path(tool: &str, cwd: &std::path::Path) -> PathBuf {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    cwd.hash(&mut h);
    let hash = format!("{:016x}", h.finish());
    // Sanitize tool name for filesystem use.
    let tool_safe: String = tool
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    home_dir().join(format!(".altevra/state/session-{tool_safe}-{hash}.txt"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // `ALTEVRA_DB_PATH` is process-global; Rust runs tests in parallel threads,
    // so concurrent set/remove on the same var races and makes assertions flaky.
    // Serialize every env-mutating test through one lock.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    // Helper to run a closure with a temporary env var set, restoring afterwards.
    fn with_env<F: FnOnce() -> R, R>(key: &str, value: Option<&str>, f: F) -> R {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var(key).ok();
        match value {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
        let result = f();
        match prev {
            Some(p) => std::env::set_var(key, p),
            None => std::env::remove_var(key),
        }
        result
    }

    #[test]
    fn default_db_path_is_home_anchored() {
        with_env("ALTEVRA_DB_PATH", None, || {
            let p = default_db_path();
            // Must be absolute and end with the canonical suffix.
            assert!(p.is_absolute(), "default_db_path must be absolute, got: {p:?}");
            assert!(
                p.to_string_lossy().ends_with(".altevra/altevra.db"),
                "must end with canonical suffix, got: {p:?}"
            );
            // Must NOT be CWD-relative (old bug: ".altevra/altevra.db").
            let cwd = std::env::current_dir().unwrap();
            assert_ne!(
                p,
                cwd.join(".altevra/altevra.db"),
                "must NOT be CWD-relative"
            );
        });
    }

    #[test]
    fn default_db_path_anchored_under_home() {
        with_env("ALTEVRA_DB_PATH", None, || {
            let p = default_db_path();
            let h = home_dir();
            assert!(
                p.starts_with(&h),
                "default_db_path must be under $HOME ({h:?}), got: {p:?}"
            );
        });
    }

    #[test]
    fn default_db_path_respects_env_override() {
        with_env("ALTEVRA_DB_PATH", Some("/tmp/altevra-custom.db"), || {
            assert_eq!(default_db_path(), PathBuf::from("/tmp/altevra-custom.db"));
        });
    }

    #[test]
    fn default_db_path_ignores_empty_env() {
        with_env("ALTEVRA_DB_PATH", Some(""), || {
            // Falls back to $HOME-anchored path.
            let p = default_db_path();
            assert!(p.is_absolute(), "empty ALTEVRA_DB_PATH must fall back to absolute home path");
        });
    }

    #[test]
    fn current_session_path_is_absolute_and_unique_per_tool_and_cwd() {
        let p1 = current_session_path("claude-code", std::path::Path::new("/home/user/proj1"));
        let p2 = current_session_path("claude-code", std::path::Path::new("/home/user/proj2"));
        let p3 = current_session_path("codex", std::path::Path::new("/home/user/proj1"));

        assert!(p1.is_absolute(), "session path must be absolute");
        assert_ne!(p1, p2, "different CWDs must produce different session files");
        assert_ne!(p1, p3, "different tools must produce different session files");
        // All are under $HOME/.altevra/state/
        let h = home_dir();
        assert!(p1.starts_with(h.join(".altevra/state")));
        assert!(p2.starts_with(home_dir().join(".altevra/state")));
    }

    // ---- default_vault_path (P0 §3 config-load fix) ----------------------
    //
    // Every test points HOME at a per-test temp dir so the REAL
    // ~/.altevra/config.toml is never read (or affected). HOME and
    // ALTEVRA_VAULT are process-global → serialize through ENV_LOCK
    // by nesting `with_env` calls.

    #[test]
    fn vault_path_from_config_reads_init_shape() {
        // The shape `altevra init` writes: [vault] path = "..."
        let cfg = "[vault]\npath = \"/data/vault\"\n";
        assert_eq!(
            vault_path_from_config(cfg),
            Some(PathBuf::from("/data/vault"))
        );
    }

    #[test]
    fn vault_path_from_config_reads_altevra_config_shape() {
        // The AltevraConfig shape: top-level vault_path = "..."
        let cfg = "vault_path = \"/data/vault2\"\nversion = \"0.1.0\"\n";
        assert_eq!(
            vault_path_from_config(cfg),
            Some(PathBuf::from("/data/vault2"))
        );
    }

    #[test]
    fn vault_path_from_config_treats_dot_and_empty_as_unset() {
        assert_eq!(vault_path_from_config("[vault]\npath = \".\"\n"), None);
        assert_eq!(vault_path_from_config("[vault]\npath = \"\"\n"), None);
        assert_eq!(vault_path_from_config("not even toml ==="), None);
        assert_eq!(vault_path_from_config("[vault]\nother = 1\n"), None);
    }

    #[test]
    fn vault_path_from_config_expands_tilde() {
        let p = vault_path_from_config("[vault]\npath = \"~/Obsidian/Imperium\"\n").unwrap();
        assert!(p.is_absolute(), "~/ must expand to an absolute path: {p:?}");
        assert!(p.ends_with("Obsidian/Imperium"));
        assert!(!p.to_string_lossy().contains('~'), "no literal ~ survives");
    }

    #[test]
    fn default_vault_path_prefers_config_over_env_and_dot() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join(".altevra")).unwrap();
        std::fs::write(
            tmp.path().join(".altevra/config.toml"),
            "[vault]\npath = \"/configured/vault\"\n",
        )
        .unwrap();
        with_env("HOME", Some(&tmp.path().to_string_lossy()), || {
            // Config wins even when the env var is also set.
            std::env::set_var("ALTEVRA_VAULT", "/env/vault");
            let p = default_vault_path();
            std::env::remove_var("ALTEVRA_VAULT");
            assert_eq!(p, PathBuf::from("/configured/vault"));
        });
    }

    #[test]
    fn default_vault_path_falls_back_to_env_then_dot() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Init placeholder "." in config → treated as unset → env wins.
        std::fs::create_dir_all(tmp.path().join(".altevra")).unwrap();
        std::fs::write(
            tmp.path().join(".altevra/config.toml"),
            "[vault]\npath = \".\"\n",
        )
        .unwrap();
        with_env("HOME", Some(&tmp.path().to_string_lossy()), || {
            std::env::set_var("ALTEVRA_VAULT", "/env/vault");
            let p = default_vault_path();
            std::env::remove_var("ALTEVRA_VAULT");
            assert_eq!(p, PathBuf::from("/env/vault"));

            // Neither config nor env → legacy "." behavior.
            let p = default_vault_path();
            assert_eq!(p, PathBuf::from("."));
        });
    }

    #[test]
    fn no_cwd_relative_altevra_paths_in_path_helpers() {
        // Static scan: none of our exported path-returning functions
        // should ever return a path equal to CWD/.altevra/...
        // We test the two most critical ones.
        with_env("ALTEVRA_DB_PATH", None, || {
            let cwd = std::env::current_dir().unwrap();
            let db = default_db_path();
            let pid = default_brain_pid_path();
            let wpid = default_watcher_pid_path();
            assert_ne!(db, cwd.join(".altevra/altevra.db"), "DB path must not be CWD-relative");
            assert_ne!(pid, cwd.join(".altevra/brain.pid"), "brain PID must not be CWD-relative");
            assert_ne!(wpid, cwd.join(".altevra/watcher.pid"), "watcher PID must not be CWD-relative");
        });
    }
}

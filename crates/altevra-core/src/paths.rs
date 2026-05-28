//! Canonical filesystem paths used across the Altevra workspace.
//!
//! Single source of truth for the local SQLite database location. CLI args,
//! brain scheduler, and MCP tools all import the same default so we never
//! drift between modules.
//!
//! Override via `ALTEVRA_DB_PATH` env var, useful for non-standard setups
//! (e.g. running multiple Altevra installs against separate stores).

use std::path::PathBuf;

/// The repo-relative default path for the Altevra SQLite database.
/// Kept as a `&str` for use in `#[arg(default_value = ...)]` attributes
/// (clap requires a `&'static str` there).
pub const DEFAULT_DB_PATH: &str = ".altevra/altevra.db";

/// Returns the active DB path, respecting `ALTEVRA_DB_PATH` when set.
pub fn default_db_path() -> PathBuf {
    std::env::var("ALTEVRA_DB_PATH")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_DB_PATH))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper to run a closure with a temporary env var set, restoring afterwards.
    fn with_env<F: FnOnce() -> R, R>(key: &str, value: Option<&str>, f: F) -> R {
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
    fn default_db_path_falls_back_to_constant() {
        with_env("ALTEVRA_DB_PATH", None, || {
            assert_eq!(default_db_path(), PathBuf::from(DEFAULT_DB_PATH));
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
            assert_eq!(default_db_path(), PathBuf::from(DEFAULT_DB_PATH));
        });
    }
}

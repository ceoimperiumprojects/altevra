//! `altevra auth` — one command for the auth lanes.
//!
//! * `altevra auth`              → status table: Codex OAuth lane (~/.codex/auth.json)
//!   + local Ollama lane (localhost:11434), and whether each is active in Altevra.
//! * `altevra auth codex`        → activate Codex OAuth: if tokens already exist, flip
//!   `[llm] reasoning_mode = "codex_oauth"` (surgical toml_edit write — every other
//!   section, including `[vault]` and `[llm.local_private]`, is preserved
//!   byte-for-byte). If not logged in, spawn `codex login` interactively first.
//! * `altevra auth ollama`       → configure `[llm.local_private]` by reusing the
//!   `altevra llm use ollama` preset code path, then verify the model is pulled.
//!
//! Hermetic-test seams: the Codex home dir is overridable via `ALTEVRA_CODEX_HOME`
//! (or `CODEX_HOME`) and every core fn takes paths as params, so tests never touch
//! the real `~/.codex` or `~/.altevra`.

use altevra_core::config::{AltevraConfig, ReasoningMode};
use altevra_core::paths::home_dir;
use chrono::{DateTime, Utc};
use clap::{Args, Subcommand};
use std::path::{Path, PathBuf};

use crate::commands::config::load_config;

/// Token freshness horizon — older than this and we nudge toward `codex login`.
const STALE_AFTER_DAYS: i64 = 7;

#[derive(Args)]
pub struct AuthArgs {
    #[command(subcommand)]
    pub command: Option<AuthCommands>,

    /// Directory whose `.altevra/config.toml` is inspected/written
    /// (default: `$HOME` — the global Altevra config)
    #[arg(long)]
    pub repo: Option<PathBuf>,
}

#[derive(Subcommand)]
pub enum AuthCommands {
    /// Activate Codex OAuth as Altevra's reasoning lane (spawns `codex login` if needed)
    Codex(CodexArgs),
    /// Configure the local Ollama lane ([llm.local_private]) — reuses `altevra llm use ollama`
    Ollama(OllamaArgs),
}

#[derive(Args)]
pub struct CodexArgs {
    /// Report only — never writes config, never spawns `codex login`
    #[arg(long)]
    pub status: bool,

    /// Directory whose `.altevra/config.toml` is written (default: `$HOME`)
    #[arg(long)]
    pub repo: Option<PathBuf>,
}

#[derive(Args)]
pub struct OllamaArgs {
    /// Model for [llm.local_private] (default: qwen2.5)
    #[arg(long)]
    pub model: Option<String>,

    /// Directory whose `.altevra/config.toml` is written (default: `$HOME`)
    #[arg(long)]
    pub repo: Option<PathBuf>,
}

pub async fn run(args: AuthArgs) -> anyhow::Result<()> {
    let codex_home = codex_home_from_env();
    match args.command {
        None => run_status_at(&resolve_repo(args.repo), &codex_home).await,
        Some(AuthCommands::Codex(a)) => {
            let repo = resolve_repo(a.repo);
            if a.status {
                // Report-only: never writes, never spawns.
                render_codex_lane(&read_codex_auth(&codex_home), &load_config(&repo));
                return Ok(());
            }
            // Config write path respects the maintenance lock (non-fatal refuse).
            if crate::commands::brain::refuse_if_maintenance_locked("auth codex") {
                return Ok(());
            }
            run_codex_at(&repo, &codex_home, "codex")
        }
        Some(AuthCommands::Ollama(a)) => {
            if crate::commands::brain::refuse_if_maintenance_locked("auth ollama") {
                return Ok(());
            }
            run_ollama_at(resolve_repo(a.repo), a.model).await
        }
    }
}

// ---------------------------------------------------------------------------
// `altevra auth` (no args) — status table
// ---------------------------------------------------------------------------

async fn run_status_at(repo: &Path, codex_home: &Path) -> anyhow::Result<()> {
    let cfg = load_config(repo);
    println!("altevra auth — lane status");
    println!("──────────────────────────");
    render_codex_lane(&read_codex_auth(codex_home), &cfg);
    render_local_lane(&cfg).await;
    println!();
    println!("hint: `altevra auth codex` to set up / activate Codex OAuth");
    Ok(())
}

fn render_codex_lane(auth: &CodexAuthStatus, cfg: &AltevraConfig) {
    println!("codex:");
    if !auth.exists {
        println!("  auth.json    : not found — not logged in (run `altevra auth codex`)");
    } else {
        println!(
            "  auth_mode    : {}",
            auth.auth_mode.as_deref().unwrap_or("(unknown)")
        );
        let tokens = match (auth.tokens_present, auth.api_key_present) {
            (true, _) => "present".to_string(),
            (false, true) => "absent (OPENAI_API_KEY set instead)".to_string(),
            (false, false) => "absent".to_string(),
        };
        println!("  tokens       : {tokens}");
        match auth.last_refresh {
            Some(t) => {
                let age = Utc::now().signed_duration_since(t);
                if auth.is_stale(Utc::now()) {
                    println!(
                        "  last_refresh : {} ago ⚠ stale (> {STALE_AFTER_DAYS} days — consider `codex login`)",
                        humanize_age(age)
                    );
                } else {
                    println!("  last_refresh : {} ago", humanize_age(age));
                }
            }
            None => println!("  last_refresh : (unknown)"),
        }
    }
    if cfg.llm.reasoning_mode == ReasoningMode::CodexOauth {
        println!("  altevra      : ACTIVE (llm.reasoning_mode = codex_oauth)");
    } else {
        println!(
            "  altevra      : INACTIVE (llm.reasoning_mode = {})",
            cfg.llm.reasoning_mode.as_str()
        );
    }
}

async fn render_local_lane(cfg: &AltevraConfig) {
    println!("local (ollama):");
    let (root, model) = match cfg.llm.local_private.as_ref() {
        Some(lp) => (
            lp.base_url
                .as_deref()
                .map(ollama_root)
                .unwrap_or_else(|| "http://localhost:11434".to_string()),
            lp.model.clone(),
        ),
        None => ("http://localhost:11434".to_string(), None),
    };
    match probe_ollama_tags(&root).await {
        Ok(names) => {
            println!("  server       : reachable at {root} ✓");
            match model.as_deref().filter(|m| !m.is_empty()) {
                Some(m) if model_available(&names, m) => {
                    println!("  model        : {m} ✓ available");
                }
                Some(m) => {
                    println!("  model        : {m} ✗ not pulled — run `ollama pull {m}`");
                }
                None => println!("  model        : (no llm.local_private model configured)"),
            }
        }
        Err(_) => {
            println!("  server       : not reachable at {root} — start with `ollama serve`");
        }
    }
    if cfg.llm.local_private.is_some() {
        println!("  altevra      : CONFIGURED ([llm.local_private] set)");
    } else {
        println!("  altevra      : NOT CONFIGURED — `altevra auth ollama` to set up");
    }
}

// ---------------------------------------------------------------------------
// `altevra auth codex`
// ---------------------------------------------------------------------------

/// Core codex activation flow. Takes `codex_home` + `codex_bin` as params so
/// tests can use fixtures and never spawn the real `codex login`.
fn run_codex_at(repo: &Path, codex_home: &Path, codex_bin: &str) -> anyhow::Result<()> {
    let mut auth = read_codex_auth(codex_home);
    if !auth.tokens_present {
        spawn_codex_login(codex_bin)?;
        auth = read_codex_auth(codex_home);
        if !auth.tokens_present {
            anyhow::bail!(
                "`codex login` finished but {} still has no tokens — not activating codex_oauth",
                codex_home.join("auth.json").display()
            );
        }
    }
    activate_codex_mode(repo)?;
    println!("✓ Codex OAuth active (kao Hermes) — llm.reasoning_mode = codex_oauth");
    println!();
    render_codex_lane(&auth, &load_config(repo));
    Ok(())
}

/// Spawn `codex login` interactively (inherited stdio → browser/device flow works).
/// A missing binary is reported with an install instruction and a non-zero exit.
fn spawn_codex_login(codex_bin: &str) -> anyhow::Result<()> {
    println!("No Codex tokens found — launching `{codex_bin} login` (browser/device flow)…");
    match std::process::Command::new(codex_bin).arg("login").status() {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => anyhow::bail!("`codex login` exited with {s} — not activating codex_oauth"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("codex binary not found in PATH.");
            eprintln!("Install it first:  npm install -g @openai/codex");
            eprintln!("then re-run:       altevra auth codex");
            anyhow::bail!("codex binary not found")
        }
        Err(e) => Err(e.into()),
    }
}

/// Set `[llm] reasoning_mode = "codex_oauth"` in `<repo>/.altevra/config.toml`.
///
/// Uses a surgical `toml_edit` write so EVERY other byte of the file —
/// `[vault]`, `[llm.local_private]`, comments, formatting — is preserved
/// exactly. (A struct round-trip through `AltevraConfig` would silently drop
/// unknown sections such as the `[vault]` table written by `altevra init`.)
/// A missing config file starts from the serialized `AltevraConfig` defaults,
/// same shape `altevra llm use` / `altevra config set` write.
pub(crate) fn activate_codex_mode(repo: &Path) -> anyhow::Result<PathBuf> {
    let path = repo.join(".altevra/config.toml");
    let base = if path.exists() {
        std::fs::read_to_string(&path)?
    } else {
        toml::to_string_pretty(&AltevraConfig::default())?
    };
    let mut doc: toml_edit::DocumentMut = base.parse().map_err(|e| {
        anyhow::anyhow!(
            "{} is not valid TOML ({e}) — fix it before running `altevra auth codex`",
            path.display()
        )
    })?;
    doc["llm"]["reasoning_mode"] = toml_edit::value("codex_oauth");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, doc.to_string())?;
    Ok(path)
}

// ---------------------------------------------------------------------------
// `altevra auth ollama`
// ---------------------------------------------------------------------------

/// Reuses the `altevra llm use ollama` preset code path (no duplicated config
/// logic), then probes `/api/tags` to tell the user whether the configured
/// model is actually pulled.
async fn run_ollama_at(repo: PathBuf, model: Option<String>) -> anyhow::Result<()> {
    crate::commands::llm::run_use(crate::commands::llm::UseArgs {
        preset: "ollama".into(),
        model,
        port: None,
        repo: repo.clone(),
    })
    .await?;

    let cfg = load_config(&repo);
    if let Some(lp) = cfg.llm.local_private.as_ref() {
        let root = lp
            .base_url
            .as_deref()
            .map(ollama_root)
            .unwrap_or_else(|| "http://localhost:11434".to_string());
        if let (Ok(names), Some(m)) = (probe_ollama_tags(&root).await, lp.model.as_deref()) {
            if !m.is_empty() && !model_available(&names, m) {
                println!("model {m} is not pulled yet — run `ollama pull {m}`");
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Codex auth.json parsing
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub(crate) struct CodexAuthStatus {
    pub exists: bool,
    pub auth_mode: Option<String>,
    pub tokens_present: bool,
    pub api_key_present: bool,
    pub last_refresh: Option<DateTime<Utc>>,
}

impl CodexAuthStatus {
    /// Tokens older than [`STALE_AFTER_DAYS`] (or with unknown refresh time
    /// while tokens are present) are flagged stale.
    pub fn is_stale(&self, now: DateTime<Utc>) -> bool {
        match self.last_refresh {
            Some(t) => now.signed_duration_since(t).num_days() > STALE_AFTER_DAYS,
            None => self.tokens_present,
        }
    }
}

/// Best-effort parse of `<codex_home>/auth.json`. Never panics; any malformed
/// field degrades to "absent/unknown" — the status table stays renderable.
pub(crate) fn read_codex_auth(codex_home: &Path) -> CodexAuthStatus {
    let path = codex_home.join("auth.json");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return CodexAuthStatus::default();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) else {
        return CodexAuthStatus {
            exists: true,
            ..Default::default()
        };
    };
    let tokens_present = v
        .get("tokens")
        .and_then(|t| t.as_object())
        .map(|t| {
            ["access_token", "refresh_token", "id_token"].iter().any(|k| {
                t.get(*k)
                    .and_then(|x| x.as_str())
                    .map(|s| !s.is_empty())
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    CodexAuthStatus {
        exists: true,
        auth_mode: v
            .get("auth_mode")
            .and_then(|x| x.as_str())
            .map(str::to_string),
        tokens_present,
        api_key_present: v
            .get("OPENAI_API_KEY")
            .and_then(|x| x.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        last_refresh: v
            .get("last_refresh")
            .and_then(|x| x.as_str())
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.with_timezone(&Utc)),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn resolve_repo(repo: Option<PathBuf>) -> PathBuf {
    repo.unwrap_or_else(home_dir)
}

/// Codex home dir: `ALTEVRA_CODEX_HOME` > `CODEX_HOME` (codex's own override)
/// > `~/.codex`.
fn codex_home_from_env() -> PathBuf {
    for var in ["ALTEVRA_CODEX_HOME", "CODEX_HOME"] {
        if let Ok(p) = std::env::var(var) {
            if !p.trim().is_empty() {
                return PathBuf::from(p);
            }
        }
    }
    home_dir().join(".codex")
}

/// `http://localhost:11434/v1` (the openai_compat base_url shape) → server root
/// `http://localhost:11434` for the native `/api/tags` probe.
fn ollama_root(base_url: &str) -> String {
    base_url
        .trim_end_matches('/')
        .trim_end_matches("/v1")
        .trim_end_matches('/')
        .to_string()
}

fn model_available(names: &[String], model: &str) -> bool {
    names
        .iter()
        .any(|n| n == model || n.split(':').next() == Some(model))
}

fn humanize_age(d: chrono::Duration) -> String {
    let secs = d.num_seconds().max(0);
    if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

/// Probe Ollama's native `/api/tags` with a ~1s timeout; returns model names
/// on 2xx, an error otherwise. Informational only — never load-bearing.
async fn probe_ollama_tags(root: &str) -> anyhow::Result<Vec<String>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(1))
        .build()?;
    let url = format!("{}/api/tags", root.trim_end_matches('/'));
    let resp = client.get(&url).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("probe HTTP {}", resp.status());
    }
    let v: serde_json::Value = resp.json().await?;
    let mut names = Vec::new();
    if let Some(arr) = v.get("models").and_then(|m| m.as_array()) {
        for item in arr {
            if let Some(n) = item.get("name").and_then(|x| x.as_str()) {
                names.push(n.to_string());
            }
        }
    }
    Ok(names)
}

// ---------------------------------------------------------------------------
// Tests — fully hermetic: fixture auth.json + config.toml in TempDirs; the
// real ~/.codex and ~/.altevra are never read or written; `codex login` is
// never spawned (the missing-binary test uses a guaranteed-nonexistent path).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Mirror of the real ~/.codex/auth.json shape (keys only, dummy values).
    fn write_auth_fixture(codex_home: &Path, last_refresh: &str, with_tokens: bool) {
        std::fs::create_dir_all(codex_home).unwrap();
        let tokens = if with_tokens {
            serde_json::json!({
                "id_token": "fixture-id",
                "access_token": "fixture-access",
                "refresh_token": "fixture-refresh",
                "account_id": "fixture-account"
            })
        } else {
            serde_json::json!({})
        };
        let doc = serde_json::json!({
            "auth_mode": "chatgpt",
            "OPENAI_API_KEY": null,
            "tokens": tokens,
            "last_refresh": last_refresh
        });
        std::fs::write(
            codex_home.join("auth.json"),
            serde_json::to_string_pretty(&doc).unwrap(),
        )
        .unwrap();
    }

    fn rfc3339_days_ago(days: i64) -> String {
        (Utc::now() - chrono::Duration::days(days)).to_rfc3339()
    }

    const CONFIG_FIXTURE: &str = r#"vault_path = "/tmp/v"
version = "0.3.0"

[database]
url = "sqlite:///tmp/a.db"
max_connections = 5

[vault]
path = "/home/x/vault"

[llm]
reasoning_mode = "delegated"
embedding_mode = "off"

[llm.local_private]
kind = "openai_compat"
base_url = "http://localhost:11434/v1"
model = "lfm2.5:8b"
"#;

    fn write_config_fixture(repo: &Path) -> PathBuf {
        let dir = repo.join(".altevra");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, CONFIG_FIXTURE).unwrap();
        path
    }

    // -- auth.json parsing ---------------------------------------------------

    #[test]
    fn read_codex_auth_missing_file() {
        let tmp = TempDir::new().unwrap();
        let s = read_codex_auth(tmp.path());
        assert!(!s.exists);
        assert!(!s.tokens_present);
        assert!(s.auth_mode.is_none());
    }

    #[test]
    fn read_codex_auth_with_tokens() {
        let tmp = TempDir::new().unwrap();
        write_auth_fixture(tmp.path(), &rfc3339_days_ago(0), true);
        let s = read_codex_auth(tmp.path());
        assert!(s.exists);
        assert!(s.tokens_present);
        assert!(!s.api_key_present);
        assert_eq!(s.auth_mode.as_deref(), Some("chatgpt"));
        assert!(s.last_refresh.is_some());
        assert!(!s.is_stale(Utc::now()), "fresh tokens are not stale");
    }

    #[test]
    fn read_codex_auth_empty_tokens_is_absent() {
        let tmp = TempDir::new().unwrap();
        write_auth_fixture(tmp.path(), &rfc3339_days_ago(0), false);
        let s = read_codex_auth(tmp.path());
        assert!(s.exists);
        assert!(!s.tokens_present);
    }

    #[test]
    fn read_codex_auth_stale_tokens() {
        let tmp = TempDir::new().unwrap();
        write_auth_fixture(tmp.path(), &rfc3339_days_ago(30), true);
        let s = read_codex_auth(tmp.path());
        assert!(s.tokens_present);
        assert!(s.is_stale(Utc::now()), "30-day-old refresh is stale");
    }

    #[test]
    fn read_codex_auth_nanosecond_timestamp_parses() {
        // The real codex writes RFC3339 with nanoseconds + Z.
        let tmp = TempDir::new().unwrap();
        write_auth_fixture(tmp.path(), "2026-06-09T23:09:13.156103770Z", true);
        let s = read_codex_auth(tmp.path());
        assert!(s.last_refresh.is_some());
    }

    // -- status rendering (smoke: must not panic for any fixture state) -------

    #[tokio::test]
    async fn status_renders_for_present_missing_and_stale_fixtures() {
        for (days, with_tokens, write_file) in
            [(0, true, true), (30, true, true), (0, false, true), (0, false, false)]
        {
            let repo = TempDir::new().unwrap();
            let codex = TempDir::new().unwrap();
            if write_file {
                write_auth_fixture(codex.path(), &rfc3339_days_ago(days), with_tokens);
            }
            write_config_fixture(repo.path());
            run_status_at(repo.path(), codex.path()).await.unwrap();
        }
    }

    // -- `auth codex` write path ----------------------------------------------

    #[test]
    fn codex_with_tokens_writes_mode_and_preserves_other_sections_byte_correct() {
        let repo = TempDir::new().unwrap();
        let codex = TempDir::new().unwrap();
        write_auth_fixture(codex.path(), &rfc3339_days_ago(0), true);
        let cfg_path = write_config_fixture(repo.path());

        run_codex_at(repo.path(), codex.path(), "codex-never-spawned").unwrap();

        let out = std::fs::read_to_string(&cfg_path).unwrap();
        // The ONLY byte-level change is the reasoning_mode value.
        assert_eq!(
            out,
            CONFIG_FIXTURE.replace(
                "reasoning_mode = \"delegated\"",
                "reasoning_mode = \"codex_oauth\""
            ),
            "every other section must be preserved byte-correctly"
        );
        assert!(out.contains("[vault]\npath = \"/home/x/vault\""));
        assert!(out.contains("[llm.local_private]"));
        assert!(out.contains("model = \"lfm2.5:8b\""));

        // And the typed loader agrees.
        let cfg = load_config(repo.path());
        assert_eq!(cfg.llm.reasoning_mode, ReasoningMode::CodexOauth);
        let lp = cfg.llm.local_private.expect("local_private preserved");
        assert_eq!(lp.model.as_deref(), Some("lfm2.5:8b"));
    }

    #[test]
    fn activate_creates_config_from_defaults_when_missing() {
        let repo = TempDir::new().unwrap();
        activate_codex_mode(repo.path()).unwrap();
        let cfg = load_config(repo.path());
        assert_eq!(cfg.llm.reasoning_mode, ReasoningMode::CodexOauth);
        // Default sections survived the round-trip (file parses as AltevraConfig).
        let raw =
            std::fs::read_to_string(repo.path().join(".altevra/config.toml")).unwrap();
        assert!(raw.contains("[database]"));
    }

    #[test]
    fn status_flag_never_writes() {
        // No config: rendering the codex lane must not create one.
        let repo = TempDir::new().unwrap();
        let codex = TempDir::new().unwrap();
        write_auth_fixture(codex.path(), &rfc3339_days_ago(0), true);
        render_codex_lane(&read_codex_auth(codex.path()), &load_config(repo.path()));
        assert!(
            !repo.path().join(".altevra/config.toml").exists(),
            "--status must never write"
        );

        // Existing config: bytes must be untouched.
        let cfg_path = write_config_fixture(repo.path());
        render_codex_lane(&read_codex_auth(codex.path()), &load_config(repo.path()));
        assert_eq!(std::fs::read_to_string(&cfg_path).unwrap(), CONFIG_FIXTURE);
    }

    #[test]
    fn missing_codex_binary_exits_gracefully_without_writing() {
        let repo = TempDir::new().unwrap();
        let codex = TempDir::new().unwrap(); // no auth.json → login path
        let r = run_codex_at(
            repo.path(),
            codex.path(),
            "/definitely/not/a/real/codex-binary-altevra-test",
        );
        assert!(r.is_err(), "missing binary must be a graceful error");
        assert!(
            !repo.path().join(".altevra/config.toml").exists(),
            "no config write on failed login"
        );
    }

    // -- helpers ---------------------------------------------------------------

    #[test]
    fn ollama_root_strips_v1_suffix() {
        assert_eq!(
            ollama_root("http://localhost:11434/v1"),
            "http://localhost:11434"
        );
        assert_eq!(
            ollama_root("http://localhost:11434/v1/"),
            "http://localhost:11434"
        );
        assert_eq!(
            ollama_root("http://localhost:11434"),
            "http://localhost:11434"
        );
    }

    #[test]
    fn model_available_matches_exact_and_tagged() {
        let names = vec!["lfm2.5:8b".to_string(), "qwen2.5:latest".to_string()];
        assert!(model_available(&names, "lfm2.5:8b"));
        assert!(model_available(&names, "qwen2.5")); // bare name matches tagged
        assert!(!model_available(&names, "llama3"));
    }
}

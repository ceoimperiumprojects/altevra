//! `altevra llm use <preset>` — one-shot LLM presets.
//!
//! Saves users from hand-editing `.altevra/config.toml`. Each preset writes the
//! correct `[llm]` shape (mode + provider table) for a common configuration:
//!
//! * `ollama`        → llm.local_private = openai_compat @ localhost:11434, model qwen2.5 (default)
//! * `vllm`          → llm.local_private = openai_compat @ localhost:<port>, model = <name>
//! * `codex`         → llm.reasoning_mode = codex_oauth + clears llm.local_private
//! * `local-first`   → llm.reasoning_mode = codex_oauth AND requires llm.local_private set
//!   (signals "cloud reasoner for non-personal; local for high-water")
//!
//! For the local-server presets we PROBE the configured `/v1/models` with a
//! tiny timeout: the result is informational only — the config is saved either
//! way, so a user can configure ahead of `ollama serve` starting.

use altevra_core::config::{AltevraConfig, ProviderSettings, ReasoningMode};
use clap::{Args, Subcommand};
use std::path::{Path, PathBuf};

use crate::commands::config::load_config;

#[derive(Subcommand)]
pub enum LlmCommands {
    /// Apply a one-shot LLM preset (writes config.toml)
    Use(UseArgs),
}

#[derive(Args)]
pub struct UseArgs {
    /// Preset name: `ollama` | `vllm` | `codex` | `local-first`
    pub preset: String,

    /// Model name (preset-specific; defaults: ollama→qwen2.5, vllm→"")
    #[arg(long)]
    pub model: Option<String>,

    /// vLLM port (default 8000, used only with `vllm`)
    #[arg(long)]
    pub port: Option<u16>,

    #[arg(long, default_value = ".")]
    pub repo: PathBuf,
}

pub async fn run(cmd: LlmCommands) -> anyhow::Result<()> {
    match cmd {
        LlmCommands::Use(args) => run_use(args).await,
    }
}

pub(crate) async fn run_use(args: UseArgs) -> anyhow::Result<()> {
    let mut cfg = load_config(&args.repo);
    let before_mode = cfg.llm.reasoning_mode.as_str().to_string();
    let before_local = describe_local_private(&cfg);

    match args.preset.as_str() {
        "ollama" => apply_ollama(&mut cfg, args.model.as_deref()).await?,
        "vllm" => apply_vllm(&mut cfg, args.port, args.model.as_deref()).await?,
        "codex" => apply_codex(&mut cfg),
        "local-first" => apply_local_first(&mut cfg)?,
        other => anyhow::bail!(
            "unknown preset: {other}\nValid presets: ollama | vllm | codex | local-first"
        ),
    }

    save_config(&args.repo, &cfg)?;

    // Two-line summary: what changed.
    println!(
        "reasoning_mode : {} -> {}",
        before_mode,
        cfg.llm.reasoning_mode.as_str()
    );
    println!(
        "local_private  : {} -> {}",
        before_local,
        describe_local_private(&cfg)
    );
    Ok(())
}

/// `ollama` preset — wires openai_compat at the standard 11434 endpoint, probes it,
/// prints a hint regardless of result. The probe never fails the command.
async fn apply_ollama(cfg: &mut AltevraConfig, model: Option<&str>) -> anyhow::Result<()> {
    let base_url = "http://localhost:11434/v1".to_string();
    let model = model.unwrap_or("qwen2.5").to_string();
    cfg.llm.local_private = Some(ProviderSettings {
        kind: Some("openai_compat".into()),
        base_url: Some(base_url.clone()),
        model: Some(model),
        secret_key: None,
    });

    match probe_models(&base_url).await {
        Ok(models) => {
            println!("Ollama detected ✓");
            if !models.is_empty() {
                let preview: Vec<String> = models.into_iter().take(5).collect();
                println!("  available models: {}", preview.join(", "));
            }
        }
        Err(_) => {
            println!(
                "Ollama not reachable at localhost:11434 — config saved anyway; \
                 start it with `ollama serve`"
            );
        }
    }
    Ok(())
}

/// `vllm` preset — same shape as ollama, just a different port (default 8000) and
/// no preset model (the user almost always wants their served model name).
async fn apply_vllm(
    cfg: &mut AltevraConfig,
    port: Option<u16>,
    model: Option<&str>,
) -> anyhow::Result<()> {
    let port = port.unwrap_or(8000);
    let base_url = format!("http://localhost:{port}/v1");
    let model = model.unwrap_or("").to_string();
    cfg.llm.local_private = Some(ProviderSettings {
        kind: Some("openai_compat".into()),
        base_url: Some(base_url.clone()),
        model: Some(model),
        secret_key: None,
    });

    match probe_models(&base_url).await {
        Ok(models) => {
            println!("vLLM detected ✓ at {base_url}");
            if !models.is_empty() {
                let preview: Vec<String> = models.into_iter().take(5).collect();
                println!("  available models: {}", preview.join(", "));
            }
        }
        Err(_) => {
            println!(
                "vLLM not reachable at localhost:{port} — config saved anyway; \
                 start it with `python -m vllm.entrypoints.openai.api_server --port {port}`"
            );
        }
    }
    Ok(())
}

/// `codex` preset — cloud reasoning via ChatGPT Plus OAuth. Clears any local_private
/// table so the user is unambiguously asking for cloud reasoning.
fn apply_codex(cfg: &mut AltevraConfig) {
    cfg.llm.reasoning_mode = ReasoningMode::CodexOauth;
    cfg.llm.local_private = None;
}

/// `local-first` — codex_oauth for non-personal reasoning AND local_private must be
/// configured (so high-water content auto-falls-back to local, see TASK 3). We only
/// flip the reasoning mode; the user must have already configured local_private
/// (typically via the `ollama` or `vllm` preset, or `config set`).
fn apply_local_first(cfg: &mut AltevraConfig) -> anyhow::Result<()> {
    if cfg.llm.local_private.is_none() {
        anyhow::bail!(
            "local-first requires llm.local_private to be configured first. \
             Try: `altevra llm use ollama` or `altevra config set llm.local_private.kind …`"
        );
    }
    cfg.llm.reasoning_mode = ReasoningMode::CodexOauth;
    Ok(())
}

fn describe_local_private(cfg: &AltevraConfig) -> String {
    match cfg.llm.local_private.as_ref() {
        None => "(none)".into(),
        Some(p) => format!(
            "{} @ {} (model={})",
            p.kind.as_deref().unwrap_or("?"),
            p.base_url.as_deref().unwrap_or("?"),
            p.model.as_deref().unwrap_or("?")
        ),
    }
}

fn config_path(repo: &Path) -> PathBuf {
    repo.join(".altevra/config.toml")
}

fn save_config(repo: &Path, cfg: &AltevraConfig) -> anyhow::Result<()> {
    let path = config_path(repo);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = toml::to_string_pretty(cfg)?;
    std::fs::write(&path, content)?;
    Ok(())
}

/// Quick best-effort probe of `<base_url>/models` to confirm an OpenAI-compatible
/// server is reachable. 2-second timeout; never blocks the command — returns the
/// model id list on 2xx, an error otherwise.
async fn probe_models(base_url: &str) -> anyhow::Result<Vec<String>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()?;
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let resp = client.get(&url).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("probe HTTP {}", resp.status());
    }
    let v: serde_json::Value = resp.json().await?;
    let mut ids = Vec::new();
    if let Some(arr) = v.get("data").and_then(|d| d.as_array()) {
        for item in arr {
            if let Some(id) = item.get("id").and_then(|x| x.as_str()) {
                ids.push(id.to_string());
            }
        }
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn ollama_preset_writes_localhost_11434_qwen() {
        let tmp = TempDir::new().unwrap();
        // Even with no Ollama running locally the preset writes config (probe is
        // informational, never load-bearing).
        run_use(UseArgs {
            preset: "ollama".into(),
            model: None,
            port: None,
            repo: tmp.path().to_path_buf(),
        })
        .await
        .unwrap();

        let cfg = load_config(tmp.path());
        let lp = cfg.llm.local_private.expect("local_private set");
        assert_eq!(lp.kind.as_deref(), Some("openai_compat"));
        assert_eq!(lp.base_url.as_deref(), Some("http://localhost:11434/v1"));
        assert_eq!(lp.model.as_deref(), Some("qwen2.5"));
        assert!(lp.secret_key.is_none());
    }

    #[tokio::test]
    async fn ollama_preset_respects_model_flag() {
        let tmp = TempDir::new().unwrap();
        run_use(UseArgs {
            preset: "ollama".into(),
            model: Some("llama3.2".into()),
            port: None,
            repo: tmp.path().to_path_buf(),
        })
        .await
        .unwrap();
        let cfg = load_config(tmp.path());
        assert_eq!(
            cfg.llm.local_private.unwrap().model.as_deref(),
            Some("llama3.2")
        );
    }

    #[tokio::test]
    async fn vllm_preset_uses_port_8000_by_default() {
        let tmp = TempDir::new().unwrap();
        run_use(UseArgs {
            preset: "vllm".into(),
            model: Some("Qwen/Qwen2.5-7B-Instruct".into()),
            port: None,
            repo: tmp.path().to_path_buf(),
        })
        .await
        .unwrap();
        let cfg = load_config(tmp.path());
        let lp = cfg.llm.local_private.unwrap();
        assert_eq!(lp.base_url.as_deref(), Some("http://localhost:8000/v1"));
        assert_eq!(lp.model.as_deref(), Some("Qwen/Qwen2.5-7B-Instruct"));
    }

    #[tokio::test]
    async fn vllm_preset_respects_custom_port() {
        let tmp = TempDir::new().unwrap();
        run_use(UseArgs {
            preset: "vllm".into(),
            model: None,
            port: Some(8123),
            repo: tmp.path().to_path_buf(),
        })
        .await
        .unwrap();
        let cfg = load_config(tmp.path());
        assert_eq!(
            cfg.llm.local_private.unwrap().base_url.as_deref(),
            Some("http://localhost:8123/v1")
        );
    }

    #[tokio::test]
    async fn codex_preset_sets_oauth_and_clears_local() {
        let tmp = TempDir::new().unwrap();
        // Seed an existing local_private to prove `codex` clears it.
        run_use(UseArgs {
            preset: "ollama".into(),
            model: None,
            port: None,
            repo: tmp.path().to_path_buf(),
        })
        .await
        .unwrap();
        run_use(UseArgs {
            preset: "codex".into(),
            model: None,
            port: None,
            repo: tmp.path().to_path_buf(),
        })
        .await
        .unwrap();
        let cfg = load_config(tmp.path());
        assert_eq!(cfg.llm.reasoning_mode, ReasoningMode::CodexOauth);
        assert!(cfg.llm.local_private.is_none());
    }

    #[tokio::test]
    async fn local_first_requires_local_private_configured() {
        let tmp = TempDir::new().unwrap();
        // Without local_private — must error.
        let r = run_use(UseArgs {
            preset: "local-first".into(),
            model: None,
            port: None,
            repo: tmp.path().to_path_buf(),
        })
        .await;
        assert!(r.is_err());

        // After ollama preset — works and flips to codex_oauth.
        run_use(UseArgs {
            preset: "ollama".into(),
            model: None,
            port: None,
            repo: tmp.path().to_path_buf(),
        })
        .await
        .unwrap();
        run_use(UseArgs {
            preset: "local-first".into(),
            model: None,
            port: None,
            repo: tmp.path().to_path_buf(),
        })
        .await
        .unwrap();
        let cfg = load_config(tmp.path());
        assert_eq!(cfg.llm.reasoning_mode, ReasoningMode::CodexOauth);
        assert!(cfg.llm.local_private.is_some());
    }

    #[tokio::test]
    async fn unknown_preset_errors() {
        let tmp = TempDir::new().unwrap();
        let r = run_use(UseArgs {
            preset: "nope".into(),
            model: None,
            port: None,
            repo: tmp.path().to_path_buf(),
        })
        .await;
        assert!(r.is_err());
    }
}

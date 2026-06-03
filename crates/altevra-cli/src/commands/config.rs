use altevra_core::config::{AltevraConfig, EmbeddingMode, ProviderSettings, ReasoningMode};
use clap::{Args, Subcommand};
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Show current configuration
    Show(ConfigShowArgs),
    /// Get a single config value by key
    Get(ConfigGetArgs),
    /// Set a config value
    Set(ConfigSetArgs),
}

#[derive(Args)]
pub struct ConfigShowArgs {
    #[arg(long)]
    pub json: bool,
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,
}

#[derive(Args)]
pub struct ConfigGetArgs {
    /// Config key (vault_path, database.url, database.max_connections)
    pub key: String,
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,
}

#[derive(Args)]
pub struct ConfigSetArgs {
    /// Config key to set
    pub key: String,
    /// New value
    pub value: String,
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,
}

pub async fn run(cmd: ConfigCommands) -> anyhow::Result<()> {
    match cmd {
        ConfigCommands::Show(args) => run_show(args).await,
        ConfigCommands::Get(args) => run_get(args).await,
        ConfigCommands::Set(args) => run_set(args).await,
    }
}

fn config_path(repo: &Path) -> PathBuf {
    repo.join(".altevra/config.toml")
}

pub fn load_config(repo: &Path) -> AltevraConfig {
    let path = config_path(repo);
    if path.exists() {
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        toml::from_str(&content).unwrap_or_default()
    } else {
        AltevraConfig::default()
    }
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

async fn run_show(args: ConfigShowArgs) -> anyhow::Result<()> {
    let cfg = load_config(&args.repo);
    if args.json {
        println!("{}", serde_json::to_string_pretty(&cfg)?);
    } else {
        println!("vault_path               = {}", cfg.vault_path);
        println!("version                  = {}", cfg.version);
        println!("database.url             = {}", cfg.database.url);
        println!(
            "database.max_connections = {}",
            cfg.database.max_connections
        );
        println!(
            "llm.reasoning_mode       = {}",
            cfg.llm.reasoning_mode.as_str()
        );
        println!(
            "llm.embedding_mode       = {}",
            cfg.llm.embedding_mode.as_str()
        );
        println!(
            "llm.codex_model          = {}",
            cfg.llm.codex_model.as_deref().unwrap_or("(default)")
        );
        if let Some(lp) = cfg.llm.local_private.as_ref() {
            println!(
                "llm.local_private.kind       = {}",
                lp.kind.as_deref().unwrap_or("(unset)")
            );
            println!(
                "llm.local_private.base_url   = {}",
                lp.base_url.as_deref().unwrap_or("(unset)")
            );
            println!(
                "llm.local_private.model      = {}",
                lp.model.as_deref().unwrap_or("(unset)")
            );
            println!(
                "llm.local_private.secret_key = {}",
                lp.secret_key.as_deref().unwrap_or("(none)")
            );
        }
    }
    Ok(())
}

async fn run_get(args: ConfigGetArgs) -> anyhow::Result<()> {
    let cfg = load_config(&args.repo);
    let val = get_key(&cfg, &args.key)?;
    println!("{val}");
    Ok(())
}

fn get_key(cfg: &AltevraConfig, key: &str) -> anyhow::Result<String> {
    // For nested local_private fields, return "" when the parent table is absent so
    // `altevra config get` behaves the same as for any unset Option<String>.
    let lp_field = |f: fn(&ProviderSettings) -> Option<String>| -> String {
        cfg.llm
            .local_private
            .as_ref()
            .and_then(f)
            .unwrap_or_default()
    };
    match key {
        "vault_path" => Ok(cfg.vault_path.clone()),
        "version" => Ok(cfg.version.clone()),
        "database.url" => Ok(cfg.database.url.clone()),
        "database.max_connections" => Ok(cfg.database.max_connections.to_string()),
        "llm.reasoning_mode" => Ok(cfg.llm.reasoning_mode.as_str().to_string()),
        "llm.embedding_mode" => Ok(cfg.llm.embedding_mode.as_str().to_string()),
        "llm.codex_model" => Ok(cfg.llm.codex_model.clone().unwrap_or_default()),
        "llm.local_private.kind" => Ok(lp_field(|p| p.kind.clone())),
        "llm.local_private.base_url" => Ok(lp_field(|p| p.base_url.clone())),
        "llm.local_private.model" => Ok(lp_field(|p| p.model.clone())),
        "llm.local_private.secret_key" => Ok(lp_field(|p| p.secret_key.clone())),
        other => anyhow::bail!(
            "Unknown config key: {other}\nValid keys: vault_path, version, database.url, \
             database.max_connections, llm.reasoning_mode, llm.embedding_mode, llm.codex_model, \
             llm.local_private.kind, llm.local_private.base_url, llm.local_private.model, \
             llm.local_private.secret_key"
        ),
    }
}

async fn run_set(args: ConfigSetArgs) -> anyhow::Result<()> {
    let mut cfg = load_config(&args.repo);
    match args.key.as_str() {
        "vault_path" => cfg.vault_path = args.value.clone(),
        "database.url" => cfg.database.url = args.value.clone(),
        "database.max_connections" => {
            cfg.database.max_connections = args
                .value
                .parse()
                .map_err(|_| anyhow::anyhow!("database.max_connections must be a number"))?;
        }
        "llm.reasoning_mode" => {
            cfg.llm.reasoning_mode = ReasoningMode::parse(&args.value).ok_or_else(|| {
                anyhow::anyhow!("llm.reasoning_mode must be one of: delegated | codex_oauth | api")
            })?;
        }
        "llm.embedding_mode" => {
            cfg.llm.embedding_mode = EmbeddingMode::parse(&args.value)
                .ok_or_else(|| anyhow::anyhow!("llm.embedding_mode must be one of: off | local"))?;
        }
        "llm.codex_model" => cfg.llm.codex_model = Some(args.value.clone()),
        // Nested local_private.* — auto-instantiate the parent table on first write so
        // a user can set fields in any order. The kind is validated to the three the
        // factory understands; base_url / model / secret_key are free strings.
        "llm.local_private.kind" => {
            let v = args.value.clone();
            if !matches!(v.as_str(), "openai_compat" | "anthropic" | "gemini") {
                anyhow::bail!(
                    "llm.local_private.kind must be one of: openai_compat | anthropic | gemini"
                );
            }
            cfg.llm
                .local_private
                .get_or_insert_with(ProviderSettings::default)
                .kind = Some(v);
        }
        "llm.local_private.base_url" => {
            cfg.llm
                .local_private
                .get_or_insert_with(ProviderSettings::default)
                .base_url = Some(args.value.clone());
        }
        "llm.local_private.model" => {
            cfg.llm
                .local_private
                .get_or_insert_with(ProviderSettings::default)
                .model = Some(args.value.clone());
        }
        "llm.local_private.secret_key" => {
            cfg.llm
                .local_private
                .get_or_insert_with(ProviderSettings::default)
                .secret_key = Some(args.value.clone());
        }
        other => anyhow::bail!(
            "Unknown config key: {other}\nSettable keys: vault_path, database.url, \
             database.max_connections, llm.reasoning_mode, llm.embedding_mode, llm.codex_model, \
             llm.local_private.kind, llm.local_private.base_url, llm.local_private.model, \
             llm.local_private.secret_key"
        ),
    }
    save_config(&args.repo, &cfg)?;
    println!("Set {}: {}", args.key, args.value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_config_show_defaults_json() {
        let tmp = TempDir::new().unwrap();
        let args = ConfigShowArgs {
            json: true,
            repo: tmp.path().to_path_buf(),
        };
        run_show(args).await.unwrap();
    }

    #[tokio::test]
    async fn test_config_get_vault_path() {
        let tmp = TempDir::new().unwrap();
        let args = ConfigGetArgs {
            key: "vault_path".into(),
            repo: tmp.path().to_path_buf(),
        };
        run_get(args).await.unwrap();
    }

    #[tokio::test]
    async fn test_config_set_persists() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join(".altevra")).unwrap();

        let set = ConfigSetArgs {
            key: "vault_path".into(),
            value: "/tmp/myvault".into(),
            repo: tmp.path().to_path_buf(),
        };
        run_set(set).await.unwrap();

        let cfg = load_config(tmp.path());
        assert_eq!(cfg.vault_path, "/tmp/myvault");
    }

    #[tokio::test]
    async fn test_config_get_unknown_key_errors() {
        let tmp = TempDir::new().unwrap();
        let args = ConfigGetArgs {
            key: "nonexistent".into(),
            repo: tmp.path().to_path_buf(),
        };
        assert!(run_get(args).await.is_err());
    }

    #[tokio::test]
    async fn test_set_get_llm_reasoning_mode() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join(".altevra")).unwrap();
        run_set(ConfigSetArgs {
            key: "llm.reasoning_mode".into(),
            value: "codex_oauth".into(),
            repo: tmp.path().to_path_buf(),
        })
        .await
        .unwrap();
        let cfg = load_config(tmp.path());
        assert_eq!(cfg.llm.reasoning_mode, ReasoningMode::CodexOauth);
        // round-trips back through get_key
        assert_eq!(get_key(&cfg, "llm.reasoning_mode").unwrap(), "codex_oauth");
    }

    #[tokio::test]
    async fn test_local_private_nested_keys_roundtrip() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join(".altevra")).unwrap();

        // Set the three load-bearing local_private fields in arbitrary order; the
        // parent table is auto-instantiated on first write.
        for (k, v) in [
            ("llm.local_private.kind", "openai_compat"),
            ("llm.local_private.base_url", "http://localhost:11434/v1"),
            ("llm.local_private.model", "qwen2.5"),
        ] {
            run_set(ConfigSetArgs {
                key: k.into(),
                value: v.into(),
                repo: tmp.path().to_path_buf(),
            })
            .await
            .unwrap();
        }

        let cfg = load_config(tmp.path());
        let lp = cfg.llm.local_private.as_ref().expect("local_private set");
        assert_eq!(lp.kind.as_deref(), Some("openai_compat"));
        assert_eq!(lp.base_url.as_deref(), Some("http://localhost:11434/v1"));
        assert_eq!(lp.model.as_deref(), Some("qwen2.5"));

        // get path returns the same strings (and "" for an unset sibling).
        assert_eq!(
            get_key(&cfg, "llm.local_private.kind").unwrap(),
            "openai_compat"
        );
        assert_eq!(
            get_key(&cfg, "llm.local_private.base_url").unwrap(),
            "http://localhost:11434/v1"
        );
        assert_eq!(get_key(&cfg, "llm.local_private.model").unwrap(), "qwen2.5");
        assert_eq!(get_key(&cfg, "llm.local_private.secret_key").unwrap(), "");

        // The on-disk TOML carries a `[llm.local_private]` table with the three fields.
        let dumped =
            std::fs::read_to_string(tmp.path().join(".altevra/config.toml")).unwrap();
        assert!(
            dumped.contains("[llm.local_private]"),
            "expected [llm.local_private] table in dump:\n{dumped}"
        );
        assert!(dumped.contains("kind = \"openai_compat\""));
        assert!(dumped.contains("base_url = \"http://localhost:11434/v1\""));
        assert!(dumped.contains("model = \"qwen2.5\""));
    }

    #[tokio::test]
    async fn test_local_private_kind_rejects_unknown() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join(".altevra")).unwrap();
        let r = run_set(ConfigSetArgs {
            key: "llm.local_private.kind".into(),
            value: "made_up".into(),
            repo: tmp.path().to_path_buf(),
        })
        .await;
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn test_set_invalid_reasoning_mode_errors() {
        let tmp = TempDir::new().unwrap();
        let r = run_set(ConfigSetArgs {
            key: "llm.reasoning_mode".into(),
            value: "bogus".into(),
            repo: tmp.path().to_path_buf(),
        })
        .await;
        assert!(r.is_err());
    }
}

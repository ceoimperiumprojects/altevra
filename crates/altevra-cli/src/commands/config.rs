use altevra_core::config::{AltevraConfig, EmbeddingMode, ReasoningMode};
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
    match key {
        "vault_path" => Ok(cfg.vault_path.clone()),
        "version" => Ok(cfg.version.clone()),
        "database.url" => Ok(cfg.database.url.clone()),
        "database.max_connections" => Ok(cfg.database.max_connections.to_string()),
        "llm.reasoning_mode" => Ok(cfg.llm.reasoning_mode.as_str().to_string()),
        "llm.embedding_mode" => Ok(cfg.llm.embedding_mode.as_str().to_string()),
        "llm.codex_model" => Ok(cfg.llm.codex_model.clone().unwrap_or_default()),
        other => anyhow::bail!(
            "Unknown config key: {other}\nValid keys: vault_path, version, database.url, \
             database.max_connections, llm.reasoning_mode, llm.embedding_mode, llm.codex_model"
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
        other => anyhow::bail!(
            "Unknown config key: {other}\nSettable keys: vault_path, database.url, \
             database.max_connections, llm.reasoning_mode, llm.embedding_mode, llm.codex_model"
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

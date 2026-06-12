//! `altevra connector` — the Connector SDK surface (PLAN-EXTEND §E1.6).
//!
//!   * `list`   — every builtin connector + its config state (enabled / auth /
//!                domains) read from `~/.altevra/connectors.toml`.
//!   * `health` — per-connector reachability/credential check (no pull).
//!   * `sync`   — pull + ingest through the FULL safety stack. `--name` limits
//!                to one connector; `--dry-run` pulls + guards but persists
//!                nothing.
//!
//! All three create the config template on first run (every connector disabled)
//! and resolve auth values from the keyring by key name — never from the toml.

use altevra_adapters::connectors::{
    builtin_connectors, AuthMode, ConnectorCtx, ConnectorsConfig,
};
use altevra_db::{create_pool, run_migrations};
use altevra_secrets::SecretStore;
use clap::{Args, Subcommand};
use std::path::PathBuf;

const KEYRING_SERVICE: &str = "altevra";

#[derive(Subcommand)]
pub enum ConnectorCommands {
    /// List builtin connectors + their config state.
    List(ConnectorListArgs),
    /// Per-connector health (reachability / credential check; no pull).
    Health(ConnectorHealthArgs),
    /// Pull + ingest enabled connectors through the safety stack.
    Sync(ConnectorSyncArgs),
}

#[derive(Args)]
pub struct ConnectorListArgs {
    #[arg(long)]
    pub json: bool,
    /// Connectors config path (defaults to ~/.altevra/connectors.toml).
    #[arg(long)]
    pub config: Option<PathBuf>,
}

#[derive(Args)]
pub struct ConnectorHealthArgs {
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub config: Option<PathBuf>,
}

#[derive(Args)]
pub struct ConnectorSyncArgs {
    /// Sync only this connector.
    #[arg(long)]
    pub name: Option<String>,
    /// Pull + guard but persist nothing.
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long)]
    pub config: Option<PathBuf>,
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
}

pub async fn run(cmd: ConnectorCommands) -> anyhow::Result<()> {
    match cmd {
        ConnectorCommands::List(args) => run_list(args),
        ConnectorCommands::Health(args) => run_health(args),
        ConnectorCommands::Sync(args) => run_sync(args).await,
    }
}

fn config_path(explicit: Option<PathBuf>) -> PathBuf {
    explicit.unwrap_or_else(ConnectorsConfig::default_path)
}

fn secret_store() -> SecretStore {
    if let Ok(path) = std::env::var("ALTEVRA_SECRETS_FILE") {
        if !path.trim().is_empty() {
            let key_env = std::env::var("ALTEVRA_SECRETS_KEY_ENV")
                .unwrap_or_else(|_| "ALTEVRA_SECRETS_KEY".to_string());
            return SecretStore::new_encrypted_file(KEYRING_SERVICE, path.into(), &key_env);
        }
    }
    SecretStore::new_keyring(KEYRING_SERVICE)
}

/// Build a ctx for a connector (config row + keyring-resolved secret).
fn ctx_for(
    cfg: &ConnectorsConfig,
    store: &SecretStore,
    name: &str,
    auth_mode: AuthMode,
) -> ConnectorCtx {
    let config = cfg
        .get(name)
        .cloned()
        .unwrap_or_else(|| altevra_adapters::connectors::ConnectorConfig::default_for(name, auth_mode));
    let auth_value = if auth_mode != AuthMode::None && !config.auth_secret.trim().is_empty() {
        store.get(&config.auth_secret).ok().flatten()
    } else {
        None
    };
    ConnectorCtx { config, auth_value, now: chrono::Utc::now() }
}

fn run_list(args: ConnectorListArgs) -> anyhow::Result<()> {
    let path = config_path(args.config);
    let cfg = ConnectorsConfig::load_or_create(&path)?;
    let store = secret_store();

    let mut entries = Vec::new();
    for c in builtin_connectors() {
        let d = c.descriptor();
        let ctx = ctx_for(&cfg, &store, &d.name, d.auth_mode);
        let domains: Vec<String> = d.domains.iter().map(|x| x.to_string()).collect();
        entries.push(serde_json::json!({
            "name": d.name,
            "kind": d.kind,
            "auth_mode": d.auth_mode.as_str(),
            "domains": domains,
            "enabled": ctx.config.enabled,
            "cadence_minutes": ctx.config.cadence_minutes,
            "description": d.description,
        }));
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({
            "config_path": path.to_string_lossy(),
            "connectors": entries,
        }))?);
    } else {
        println!("{} connector(s) — config: {}", entries.len(), path.display());
        for e in &entries {
            println!(
                "  [{:>8}] {:10} {:12} {} (domains: {})",
                if e["enabled"].as_bool().unwrap_or(false) { "enabled" } else { "disabled" },
                e["name"].as_str().unwrap_or("-"),
                format!("{}/{}", e["kind"].as_str().unwrap_or("-"), e["auth_mode"].as_str().unwrap_or("-")),
                e["description"].as_str().unwrap_or(""),
                e["domains"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(",")).unwrap_or_default(),
            );
        }
    }
    Ok(())
}

fn run_health(args: ConnectorHealthArgs) -> anyhow::Result<()> {
    let path = config_path(args.config);
    let cfg = ConnectorsConfig::load_or_create(&path)?;
    let store = secret_store();

    let mut healths = Vec::new();
    for c in builtin_connectors() {
        let d = c.descriptor();
        if let Some(want) = &args.name {
            if &d.name != want {
                continue;
            }
        }
        let ctx = ctx_for(&cfg, &store, &d.name, d.auth_mode);
        let h = c.health(&ctx);
        healths.push(h);
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "health": healths }))?);
    } else if healths.is_empty() {
        println!("No matching connector.");
    } else {
        for h in &healths {
            let mark = match h.status.as_str() {
                "green" => "✓",
                "red" => "✗",
                "disabled" => "·",
                _ => "?",
            };
            println!("  {mark} {:10} [{:>12}] {}", h.name, h.status, h.detail);
        }
    }
    Ok(())
}

async fn run_sync(args: ConnectorSyncArgs) -> anyhow::Result<()> {
    // Stand down during db unify (non-fatal). Dry-run is read-only and proceeds.
    if !args.dry_run && crate::commands::brain::refuse_if_maintenance_locked("connector sync") {
        return Ok(());
    }
    let path = config_path(args.config);
    let pool = create_pool(&args.db.to_string_lossy()).await?;
    run_migrations(&pool).await?;

    let report = altevra_brain::run_connector_sync_at(
        &pool,
        &path,
        chrono::Utc::now(),
        args.name.as_deref(),
        args.dry_run,
    )
    .await?;

    if args.dry_run {
        println!("DRY-RUN — {}", report.summary());
    } else {
        println!("{}", report.summary());
    }
    for r in &report.results {
        let mark = match r.health.status.as_str() {
            "green" => "✓",
            "red" => "✗",
            "disabled" => "·",
            _ => "?",
        };
        println!(
            "  {mark} {:10} {} item(s){}  {}",
            r.name,
            r.persisted,
            if r.sightings > 0 {
                format!(", {} secret(s) redacted", r.sightings)
            } else {
                String::new()
            },
            r.health.detail,
        );
    }
    Ok(())
}

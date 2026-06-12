//! `connector_sync` brain job (PLAN-EXTEND §E1.4) — drives the Connector SDK.
//!
//! For each ENABLED connector in `~/.altevra/connectors.toml`:
//!   1. resolve its auth value from the keyring (by the config's `auth_secret`
//!      key name — the secret value never lives in the toml),
//!   2. `pull()` its items (read-only),
//!   3. `ingest_items()` them through the FULL gate stack (guard → domain floor
//!      → persist into events + object_index),
//!   4. record health.
//!
//! A failing connector flips its health red and is recorded — it NEVER aborts
//! the other connectors or other brain jobs. Disabled connectors are skipped.
//! Connectors are ALSO registered as `tool_records` rows (kind=connector,
//! source=manual) so they surface in `altevra tool list` and the registry.

use altevra_adapters::connectors::{
    builtin_connectors, ingest_items, AuthMode, Connector, ConnectorCtx, ConnectorHealth,
    ConnectorsConfig,
};
use altevra_db::{ToolRecordRow, ToolRecordsRepository};
use altevra_secrets::SecretStore;
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use std::path::Path;

/// Keyring service name connectors resolve their secrets under (matches the CLI
/// `altevra secrets` default service).
const KEYRING_SERVICE: &str = "altevra";

/// Per-connector sync result for the job summary + health.
#[derive(Debug, Clone)]
pub struct ConnectorSyncResult {
    pub name: String,
    pub health: ConnectorHealth,
    pub persisted: usize,
    pub sightings: usize,
}

/// Aggregate report.
#[derive(Debug, Clone, Default)]
pub struct ConnectorSyncReport {
    pub results: Vec<ConnectorSyncResult>,
}

impl ConnectorSyncReport {
    pub fn total_persisted(&self) -> usize {
        self.results.iter().map(|r| r.persisted).sum()
    }
    pub fn red_count(&self) -> usize {
        self.results.iter().filter(|r| r.health.status == "red").count()
    }
    pub fn enabled_count(&self) -> usize {
        self.results.iter().filter(|r| r.health.status != "disabled").count()
    }
    pub fn summary(&self) -> String {
        format!(
            "connector_sync: {} enabled, {} item(s) persisted, {} red",
            self.enabled_count(),
            self.total_persisted(),
            self.red_count(),
        )
    }
}

/// Resolve the keyring store. Honors the same encrypted-file fallback the rest
/// of Altevra uses when `ALTEVRA_SECRETS_FILE` / `ALTEVRA_SECRETS_KEY_ENV` are
/// set (tests), else the OS keyring.
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

/// Register every builtin connector as a `tool_records` row (kind=connector,
/// source=manual). Idempotent (upsert by name+kind). Returns count registered.
pub async fn register_connectors_as_tools(pool: &SqlitePool) -> anyhow::Result<usize> {
    let repo = ToolRecordsRepository::new(pool);
    let mut n = 0usize;
    for c in builtin_connectors() {
        let d = c.descriptor();
        let mut row = repo
            .get(&d.name, "connector")
            .await?
            .unwrap_or_else(|| ToolRecordRow::new(&d.name, "connector"));
        row.description = Some(d.description.clone());
        row.display_name = Some(format!("{} connector ({})", d.name, d.kind));
        row.invocation = serde_json::json!({
            "canonical": format!("altevra connector sync --name {}", d.name),
            "alternates": [],
        });
        row.categories = serde_json::json!(["connector", d.kind]);
        row.source = "manual".to_string();
        repo.upsert(&row).await?;
        n += 1;
    }
    Ok(n)
}

/// Run the connector sync over a specific config path (testable). `only` limits
/// to a single connector by name; `dry_run` pulls + guards but persists nothing.
pub async fn run_connector_sync_at(
    pool: &SqlitePool,
    config_path: &Path,
    now: DateTime<Utc>,
    only: Option<&str>,
    dry_run: bool,
) -> anyhow::Result<ConnectorSyncReport> {
    // Connectors are tools too — keep the register in sync (best-effort).
    if !dry_run {
        let _ = register_connectors_as_tools(pool).await;
    }

    let cfg = ConnectorsConfig::load_or_create(config_path).unwrap_or_default();
    let store = secret_store();
    let mut report = ConnectorSyncReport::default();

    for connector in builtin_connectors() {
        let name = connector.descriptor().name;
        if let Some(want) = only {
            if name != want {
                continue;
            }
        }
        let ctx = match build_ctx(&cfg, &store, &connector, now) {
            Some(c) => c,
            None => {
                // No config row → treat as disabled (template not yet filled).
                report.results.push(ConnectorSyncResult {
                    name: name.clone(),
                    health: ConnectorHealth::disabled(&name),
                    persisted: 0,
                    sightings: 0,
                });
                continue;
            }
        };

        if !ctx.config.enabled {
            report.results.push(ConnectorSyncResult {
                name: name.clone(),
                health: ConnectorHealth::disabled(&name),
                persisted: 0,
                sightings: 0,
            });
            continue;
        }

        // Pull + ingest. A failure NEVER aborts the loop — it just goes red.
        let (health, persisted, sightings) = sync_one(pool, &*connector, &ctx, dry_run).await;
        report.results.push(ConnectorSyncResult { name, health, persisted, sightings });
    }
    Ok(report)
}

/// Build the runtime ctx for a connector: its config row + the keyring-resolved
/// secret (None for `auth_mode = none` or when absent). Returns None if there is
/// no config row at all.
fn build_ctx(
    cfg: &ConnectorsConfig,
    store: &SecretStore,
    connector: &Box<dyn Connector>,
    now: DateTime<Utc>,
) -> Option<ConnectorCtx> {
    let d = connector.descriptor();
    let config = cfg.get(&d.name)?.clone();
    let auth_value = if d.auth_mode != AuthMode::None && !config.auth_secret.trim().is_empty() {
        // Resolve by key NAME from the keyring. A keyring error → None (red
        // health downstream), never a panic, never logged.
        store.get(&config.auth_secret).ok().flatten()
    } else {
        None
    };
    Some(ConnectorCtx { config, auth_value, now })
}

async fn sync_one(
    pool: &SqlitePool,
    connector: &dyn Connector,
    ctx: &ConnectorCtx,
    dry_run: bool,
) -> (ConnectorHealth, usize, usize) {
    let name = connector.descriptor().name;
    match connector.pull(ctx) {
        Ok(items) => {
            if dry_run {
                return (
                    ConnectorHealth::green(&name, format!("dry-run: {} item(s)", items.len())),
                    items.len(),
                    0,
                );
            }
            match ingest_items(pool, &name, &items).await {
                Ok(out) => (
                    ConnectorHealth::green(&name, out.summary()),
                    out.persisted,
                    out.total_sightings,
                ),
                Err(e) => (ConnectorHealth::red(&name, format!("ingest failed: {e}")), 0, 0),
            }
        }
        Err(e) => (ConnectorHealth::red(&name, format!("pull failed: {e}")), 0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn pool() -> SqlitePool {
        let p = altevra_db::create_pool("sqlite::memory:").await.unwrap();
        altevra_db::run_migrations(&p).await.unwrap();
        p
    }

    fn now() -> DateTime<Utc> {
        use chrono::TimeZone;
        Utc.with_ymd_and_hms(2026, 6, 12, 12, 0, 0).unwrap()
    }

    #[tokio::test]
    async fn disabled_by_default_persists_nothing() {
        let p = pool().await;
        let tmp = TempDir::new().unwrap();
        let cfg_path = tmp.path().join("connectors.toml");
        // load_or_create writes the all-disabled template.
        let report = run_connector_sync_at(&p, &cfg_path, now(), None, false)
            .await
            .unwrap();
        assert_eq!(report.total_persisted(), 0, "all connectors disabled by default");
        assert_eq!(report.enabled_count(), 0);
        // every result is 'disabled'.
        assert!(report.results.iter().all(|r| r.health.status == "disabled"));
    }

    #[tokio::test]
    async fn enabled_ics_persists_today_event_and_registers_tools() {
        let p = pool().await;
        let tmp = TempDir::new().unwrap();
        let ics = tmp.path().join("cal.ics");
        std::fs::write(
            &ics,
            "BEGIN:VEVENT\r\nUID:e1\r\nSUMMARY:Standup\r\nDTSTART:20260612T090000Z\r\nEND:VEVENT\r\n",
        )
        .unwrap();
        let cfg_path = tmp.path().join("connectors.toml");
        std::fs::write(
            &cfg_path,
            format!(
                "[ics]\nenabled = true\ncadence_minutes = 60\n[ics.params]\npath = \"{}\"\n",
                ics.to_str().unwrap()
            ),
        )
        .unwrap();

        let report = run_connector_sync_at(&p, &cfg_path, now(), Some("ics"), false)
            .await
            .unwrap();
        let ics_res = report.results.iter().find(|r| r.name == "ics").unwrap();
        assert!(ics_res.health.is_green(), "{:?}", ics_res.health);
        assert_eq!(ics_res.persisted, 1);

        // tool_records registration happened (kind=connector).
        let tools = ToolRecordsRepository::new(&p)
            .list(Some("connector"), None)
            .await
            .unwrap();
        assert!(tools.iter().any(|t| t.name == "ics" && t.source == "manual"));
    }

    #[tokio::test]
    async fn dry_run_persists_nothing() {
        let p = pool().await;
        let tmp = TempDir::new().unwrap();
        let ics = tmp.path().join("cal.ics");
        std::fs::write(
            &ics,
            "BEGIN:VEVENT\r\nUID:e1\r\nSUMMARY:S\r\nDTSTART:20260612T090000Z\r\nEND:VEVENT\r\n",
        )
        .unwrap();
        let cfg_path = tmp.path().join("connectors.toml");
        std::fs::write(
            &cfg_path,
            format!(
                "[ics]\nenabled = true\n[ics.params]\npath = \"{}\"\n",
                ics.to_str().unwrap()
            ),
        )
        .unwrap();
        let report = run_connector_sync_at(&p, &cfg_path, now(), Some("ics"), true)
            .await
            .unwrap();
        // dry-run reports the pull count but writes no object_index rows.
        let cands = altevra_db::ObjectIndexRepository::new(&p)
            .candidates(None)
            .await
            .unwrap();
        assert!(cands.is_empty(), "dry-run must not persist");
        assert_eq!(report.results.iter().find(|r| r.name == "ics").unwrap().persisted, 1);
    }
}

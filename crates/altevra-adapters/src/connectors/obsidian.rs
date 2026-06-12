//! Obsidian connector (PLAN-EXTEND §E1.5) — descriptor-only registration.
//!
//! The Obsidian vault is ALREADY ingested by the vault watcher + indexer, so
//! this connector exists purely for UNIFORMITY: it appears in `connector list`
//! and is registered as a `tool_records` row like every other connector, but it
//! pulls nothing (the real ingest path owns vault content). Health is green
//! when the vault path resolves, signalling "this surface is covered elsewhere".

use super::{
    AuthMode, Connector, ConnectorCtx, ConnectorDescriptor, ConnectorHealth, ConnectorItem,
};
use altevra_core::domain::Domain;

pub struct ObsidianConnector;

impl ObsidianConnector {
    pub fn new() -> Self {
        ObsidianConnector
    }
}

impl Default for ObsidianConnector {
    fn default() -> Self {
        Self::new()
    }
}

impl Connector for ObsidianConnector {
    fn descriptor(&self) -> ConnectorDescriptor {
        ConnectorDescriptor {
            name: "obsidian".into(),
            kind: "notes".into(),
            auth_mode: AuthMode::None,
            domains: vec![Domain::Personal, Domain::Business],
            description: "Obsidian vault (descriptor-only; content ingested by the vault watcher)"
                .into(),
        }
    }

    fn pull(&self, _ctx: &ConnectorCtx) -> anyhow::Result<Vec<ConnectorItem>> {
        // Intentionally empty: vault content is owned by the watcher/indexer.
        Ok(Vec::new())
    }

    fn health(&self, ctx: &ConnectorCtx) -> ConnectorHealth {
        if !ctx.config.enabled {
            return ConnectorHealth::disabled("obsidian");
        }
        ConnectorHealth::green("obsidian", "vault ingested by watcher (descriptor-only)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectors::config::ConnectorConfig;
    use chrono::Utc;

    #[test]
    fn descriptor_only_pulls_nothing() {
        let c = ObsidianConnector::new();
        let ctx = ConnectorCtx {
            config: ConnectorConfig::default_for("obsidian", AuthMode::None),
            auth_value: None,
            now: Utc::now(),
        };
        assert!(c.pull(&ctx).unwrap().is_empty());
        assert_eq!(c.descriptor().auth_mode, AuthMode::None);
    }
}

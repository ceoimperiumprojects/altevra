//! Connector config — `~/.altevra/connectors.toml` (PLAN-EXTEND §E1.2).
//!
//! Create-if-absent template with EVERY connector `enabled = false`. Auth values
//! are NEVER inline; the toml only ever holds `auth_secret`, the KEY NAME under
//! which the value lives in the `altevra-secrets` keyring. Per-connector knobs:
//! `enabled`, `auth_secret` (keyring key name), `cadence_minutes`, `domain`
//! (optional override of the connector's default declared domain), and a free
//! `params` table for connector-specific settings (ICS path/url, IMAP host,
//! mailbox, snippet cap, …).

use super::AuthMode;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Per-connector config row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectorConfig {
    /// Disabled by default — a connector that is off pulls nothing.
    #[serde(default)]
    pub enabled: bool,
    /// Keyring KEY NAME holding the auth value (never the value itself). Empty
    /// for `auth_mode = none`.
    #[serde(default)]
    pub auth_secret: String,
    /// How often `connector_sync` pulls this connector.
    #[serde(default = "default_cadence")]
    pub cadence_minutes: u32,
    /// Optional domain override (e.g. force calendar items to `personal`).
    #[serde(default)]
    pub domain: Option<String>,
    /// Connector-specific params (string-valued so the toml stays simple).
    #[serde(default)]
    pub params: BTreeMap<String, String>,
}

fn default_cadence() -> u32 {
    60
}

impl ConnectorConfig {
    /// A disabled-by-default config for a connector with the given auth mode.
    pub fn default_for(_name: &str, _auth: AuthMode) -> Self {
        Self {
            enabled: false,
            auth_secret: String::new(),
            cadence_minutes: default_cadence(),
            domain: None,
            params: BTreeMap::new(),
        }
    }

    /// Read a string param, trimmed; None when absent or blank.
    pub fn param(&self, key: &str) -> Option<&str> {
        self.params.get(key).map(|s| s.trim()).filter(|s| !s.is_empty())
    }
}

/// The whole `connectors.toml` document — a map of connector name → config.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConnectorsConfig {
    #[serde(flatten)]
    pub connectors: BTreeMap<String, ConnectorConfig>,
}

impl ConnectorsConfig {
    /// Default config path: `$ALTEVRA_CONNECTORS_PATH` (tests) or
    /// `$HOME/.altevra/connectors.toml`.
    pub fn default_path() -> PathBuf {
        if let Ok(p) = std::env::var("ALTEVRA_CONNECTORS_PATH") {
            if !p.trim().is_empty() {
                return PathBuf::from(p);
            }
        }
        altevra_core::home_dir().join(".altevra/connectors.toml")
    }

    /// Load the config, creating the commented template (all connectors
    /// `enabled = false`) on first run. A parse error degrades to an empty
    /// config (every connector treated as absent → unconfigured) rather than
    /// killing the sync — a broken toml must never block the brain.
    pub fn load_or_create(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, DEFAULT_CONNECTORS_TEMPLATE)?;
        }
        let raw = std::fs::read_to_string(path)?;
        match toml::from_str::<ConnectorsConfig>(&raw) {
            Ok(c) => Ok(c),
            Err(e) => {
                tracing::warn!("connectors.toml parse failed ({e}); treating as empty");
                Ok(ConnectorsConfig::default())
            }
        }
    }

    /// Config for one connector by name (None when absent).
    pub fn get(&self, name: &str) -> Option<&ConnectorConfig> {
        self.connectors.get(name)
    }
}

/// The create-if-absent template. EVERY connector ships `enabled = false`. Auth
/// is referenced by keyring key name (`auth_secret`), never inline.
pub const DEFAULT_CONNECTORS_TEMPLATE: &str = r#"# Altevra connectors — external tools that feed the second brain.
#
# SECURITY: never put a password/token/api-key in this file. The `auth_secret`
# value is the KEY NAME under which the secret lives in the Altevra keyring.
# Add the secret with:  altevra secrets set <KEY_NAME>
#
# Every connector is DISABLED by default. Flip `enabled = true` and fill the
# params to turn one on, then `altevra connector sync --name <name>`.

# ICS calendar — a local .ics file path OR a private-ICS URL (works with Google
# Calendar's "Secret address in iCal format" with ZERO OAuth). domain=personal.
[ics]
enabled = false
auth_secret = ""          # only needed if the URL itself is the secret (auth_mode=ics_url)
cadence_minutes = 60
# domain = "personal"
[ics.params]
# path = "/home/you/calendar.ics"     # local file, OR:
# url  = "https://calendar.google.com/calendar/ical/.../basic.ics"

# IMAP email headers — UNSEEN headers + a capped snippet ONLY (never bodies).
# Use a Gmail app password (auth_mode=app_password). domain=comms.
[imap]
enabled = false
auth_secret = "ALTEVRA_IMAP_APP_PASSWORD"
cadence_minutes = 30
[imap.params]
# host = "imap.gmail.com"
# port = "993"
# user = "you@gmail.com"
# mailbox = "INBOX"
# snippet_chars = "200"

# Linear — open issues assigned to the API-key viewer (GraphQL). domain=project.
[linear]
enabled = false
auth_secret = "ALTEVRA_LINEAR_API_KEY"
cadence_minutes = 120
[linear.params]
# endpoint = "https://api.linear.app/graphql"

# Obsidian vault — registered for uniformity; content is already ingested by the
# vault watcher, so this connector is descriptor-only (pulls nothing).
[obsidian]
enabled = false
cadence_minutes = 1440
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn template_creates_all_disabled() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("connectors.toml");
        let cfg = ConnectorsConfig::load_or_create(&path).unwrap();
        assert!(path.exists());
        // Every shipped connector present + disabled.
        for name in ["ics", "imap", "linear", "obsidian"] {
            let c = cfg.get(name).unwrap_or_else(|| panic!("missing {name}"));
            assert!(!c.enabled, "{name} must default disabled");
        }
    }

    #[test]
    fn template_holds_no_secret_values_only_key_names() {
        // The shipped template must reference key NAMES, never a literal secret.
        assert!(DEFAULT_CONNECTORS_TEMPLATE.contains("auth_secret"));
        // No obvious credential-looking literals.
        assert!(!DEFAULT_CONNECTORS_TEMPLATE.to_lowercase().contains("password = \""));
        assert!(!DEFAULT_CONNECTORS_TEMPLATE.contains("sk-"));
        assert!(!DEFAULT_CONNECTORS_TEMPLATE.contains("Bearer "));
    }

    #[test]
    fn broken_toml_degrades_to_empty_not_panic() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("connectors.toml");
        std::fs::write(&path, "this = is = not valid toml [[[").unwrap();
        let cfg = ConnectorsConfig::load_or_create(&path).unwrap();
        assert!(cfg.get("ics").is_none());
    }

    #[test]
    fn params_round_trip() {
        let toml = r#"
[ics]
enabled = true
cadence_minutes = 15
[ics.params]
path = "/tmp/cal.ics"
"#;
        let cfg: ConnectorsConfig = toml::from_str(toml).unwrap();
        let c = cfg.get("ics").unwrap();
        assert!(c.enabled);
        assert_eq!(c.cadence_minutes, 15);
        assert_eq!(c.param("path"), Some("/tmp/cal.ics"));
        assert_eq!(c.param("missing"), None);
    }
}

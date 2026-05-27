use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: u32,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: "postgres://localhost/altevra".to_string(),
            max_connections: 10,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AltevraConfig {
    pub database: DatabaseConfig,
    pub vault_path: String,
    pub version: String,
}

impl Default for AltevraConfig {
    fn default() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
        Self {
            database: DatabaseConfig::default(),
            vault_path: format!("{home}/.altevra/vault"),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

impl AltevraConfig {
    pub fn from_env() -> Self {
        let mut cfg = Self::default();
        if let Ok(url) = std::env::var("ALTEVRA_DATABASE_URL") {
            cfg.database.url = url;
        }
        if let Ok(path) = std::env::var("ALTEVRA_VAULT_PATH") {
            cfg.vault_path = path;
        }
        cfg
    }
}

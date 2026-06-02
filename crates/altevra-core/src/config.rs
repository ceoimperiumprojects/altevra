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

/// Which backend does the *reasoning* (resident modes, synthesis, classification).
///
/// `delegated` is the keyless default: Altevra calls no cloud LLM itself — the
/// connected tool (Claude/Cursor/Codex over MCP) does the thinking. `codex_oauth`
/// routes cheap_worker/strong_reasoner through ChatGPT (GPT-5.5) via
/// `~/.codex/auth.json`. `api` uses configured API providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningMode {
    #[default]
    Delegated,
    CodexOauth,
    Api,
}

impl ReasoningMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ReasoningMode::Delegated => "delegated",
            ReasoningMode::CodexOauth => "codex_oauth",
            ReasoningMode::Api => "api",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "delegated" => Some(ReasoningMode::Delegated),
            "codex_oauth" => Some(ReasoningMode::CodexOauth),
            "api" => Some(ReasoningMode::Api),
            _ => None,
        }
    }
}

/// Whether the optional local hybrid-search embedding lane is active. `off` (default)
/// keeps the deterministic core (tag + FTS5 + graph, R12) as the only retrieval path;
/// `local` activates the BGE-M3 dense + FTS5 + RRF opt-in layer (R15). Cloud embedding
/// is intentionally NOT a mode here — personal/high-water data must embed locally (SI-7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingMode {
    #[default]
    Off,
    Local,
}

impl EmbeddingMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            EmbeddingMode::Off => "off",
            EmbeddingMode::Local => "local",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "off" => Some(EmbeddingMode::Off),
            "local" => Some(EmbeddingMode::Local),
            _ => None,
        }
    }
}

/// Per-role provider settings. The actual API key lives in the keyring under
/// `secret_key`'s name (set via `altevra secrets set <KEY>`); the config never
/// stores a secret value.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderSettings {
    /// "openai_compat" | "anthropic" | "gemini".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Name of the keyring entry holding the API key (never the key itself).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_key: Option<String>,
}

/// The `[llm]` config section. Scalars are declared before the nested provider
/// tables so TOML serialization emits values before tables.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmConfig {
    pub reasoning_mode: ReasoningMode,
    pub embedding_mode: EmbeddingMode,
    /// Codex model when `reasoning_mode = codex_oauth` (factory default: gpt-5.5).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codex_model: Option<String>,
    /// Override for the Codex endpoint (e.g. a self-hosted OpenAI-compatible wrapper).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codex_base_url: Option<String>,
    /// Local embedder model id (feature `embedding`; default BGE-M3 in the factory).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedder_model: Option<String>,
    /// Cloud reasoning providers, used only when `reasoning_mode = api`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cheap_worker: Option<ProviderSettings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strong_reasoner: Option<ProviderSettings>,
    /// Local reasoning provider (must be localhost; SI-7). Independent of reasoning_mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_private: Option<ProviderSettings>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AltevraConfig {
    pub database: DatabaseConfig,
    pub vault_path: String,
    pub version: String,
    /// LLM provider + embedding selection. `#[serde(default)]` so existing
    /// config.toml files without an `[llm]` section still deserialize cleanly.
    #[serde(default)]
    pub llm: LlmConfig,
}

impl Default for AltevraConfig {
    fn default() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
        Self {
            database: DatabaseConfig::default(),
            vault_path: format!("{home}/.altevra/vault"),
            version: env!("CARGO_PKG_VERSION").to_string(),
            llm: LlmConfig::default(),
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
        if let Ok(m) = std::env::var("ALTEVRA_LLM_REASONING_MODE") {
            if let Some(mode) = ReasoningMode::parse(&m) {
                cfg.llm.reasoning_mode = mode;
            }
        }
        if let Ok(m) = std::env::var("ALTEVRA_LLM_EMBEDDING_MODE") {
            if let Some(mode) = EmbeddingMode::parse(&m) {
                cfg.llm.embedding_mode = mode;
            }
        }
        if let Ok(m) = std::env::var("ALTEVRA_LLM_CODEX_MODEL") {
            cfg.llm.codex_model = Some(m);
        }
        cfg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llm_config_defaults_are_delegated_and_off() {
        let c = LlmConfig::default();
        assert_eq!(c.reasoning_mode, ReasoningMode::Delegated);
        assert_eq!(c.embedding_mode, EmbeddingMode::Off);
        assert!(c.cheap_worker.is_none());
    }

    #[test]
    fn config_roundtrips_through_toml_with_llm_section() {
        let mut cfg = AltevraConfig::default();
        cfg.llm.reasoning_mode = ReasoningMode::CodexOauth;
        cfg.llm.embedding_mode = EmbeddingMode::Local;
        cfg.llm.codex_model = Some("gpt-5.5".to_string());
        cfg.llm.strong_reasoner = Some(ProviderSettings {
            kind: Some("openai_compat".to_string()),
            base_url: Some("https://api.deepseek.com/v1".to_string()),
            model: Some("deepseek-reasoner".to_string()),
            secret_key: Some("DEEPSEEK_API_KEY".to_string()),
        });
        let toml_str = toml::to_string_pretty(&cfg).expect("serialize");
        let back: AltevraConfig = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(back.llm.reasoning_mode, ReasoningMode::CodexOauth);
        assert_eq!(back.llm.embedding_mode, EmbeddingMode::Local);
        assert_eq!(back.llm.codex_model.as_deref(), Some("gpt-5.5"));
        assert_eq!(
            back.llm.strong_reasoner.unwrap().base_url.as_deref(),
            Some("https://api.deepseek.com/v1")
        );
    }

    #[test]
    fn legacy_config_without_llm_section_deserializes_to_defaults() {
        // A config.toml written before the [llm] section existed must still load,
        // preserving database/vault values and defaulting llm.
        let legacy = r#"
vault_path = "/home/x/.altevra/vault"
version = "0.3.0"

[database]
url = "sqlite:///tmp/a.db"
max_connections = 5
"#;
        let cfg: AltevraConfig = toml::from_str(legacy).expect("legacy parses");
        assert_eq!(cfg.vault_path, "/home/x/.altevra/vault");
        assert_eq!(cfg.database.max_connections, 5);
        assert_eq!(cfg.llm.reasoning_mode, ReasoningMode::Delegated);
        assert_eq!(cfg.llm.embedding_mode, EmbeddingMode::Off);
    }

    #[test]
    fn reasoning_and_embedding_mode_parse_roundtrip() {
        for m in [
            ReasoningMode::Delegated,
            ReasoningMode::CodexOauth,
            ReasoningMode::Api,
        ] {
            assert_eq!(ReasoningMode::parse(m.as_str()), Some(m));
        }
        assert_eq!(ReasoningMode::parse("garbage"), None);
        for m in [EmbeddingMode::Off, EmbeddingMode::Local] {
            assert_eq!(EmbeddingMode::parse(m.as_str()), Some(m));
        }
        assert_eq!(EmbeddingMode::parse("cloud"), None);
    }
}

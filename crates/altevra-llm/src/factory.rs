//! Builds a [`ModelRouter`] from the `[llm]` config + available credentials.
//!
//! This is the single place the three reasoning MODES (`delegated`, `codex_oauth`,
//! `api`) turn into registered providers. Zero config → an all-noop router, identical
//! to today's behavior, so the baseline stays green.
//!
//! SI-7 is enforced at THREE layers (defense in depth):
//!   1. here (factory): a cloud provider is NEVER registered for `local_private`;
//!      the `local_private` slot is only filled by a provider whose `is_local()` is true.
//!   2. [`ModelRouter::resolve`]: backstop — a non-local provider registered for
//!      `local_private` still resolves to the local fallback.
//!   3. resident mode validation: a `personal_data_allowed` mode whose role isn't
//!      `local_private` is skipped before any provider call.

use crate::{
    AnthropicProvider, ChatProvider, CodexOAuthProvider, GeminiFlashChat, ModelRole, ModelRouter,
    OpenAICompatProvider,
};
use altevra_core::config::{LlmConfig, ProviderSettings, ReasoningMode};
use std::sync::Arc;

/// Construct the runtime router. Reads credentials from the keyring/env as needed;
/// any missing credential degrades gracefully to noop (never panics).
pub fn build_router(cfg: &LlmConfig) -> ModelRouter {
    let mut router = ModelRouter::noop();

    // --- local_private: independent of reasoning_mode; only if genuinely local (SI-7 #1).
    if let Some(spec) = &cfg.local_private {
        if let Some(p) = build_provider(spec) {
            if p.is_local() {
                router = router.with_provider(ModelRole::LocalPrivate, p);
            } else {
                tracing::warn!(
                    "local_private provider '{}' is not local — refusing (SI-7); staying noop",
                    p.id()
                );
            }
        }
    }

    // --- reasoning roles (cheap_worker, strong_reasoner): depend on reasoning_mode.
    match cfg.reasoning_mode {
        ReasoningMode::Delegated => {
            // Connected tool does the thinking over MCP; reasoning roles stay noop.
        }
        ReasoningMode::CodexOauth => match CodexOAuthProvider::from_default_auth() {
            Ok(mut c) => {
                if let Some(m) = &cfg.codex_model {
                    c = c.with_model(m);
                }
                if let Some(u) = &cfg.codex_base_url {
                    c = c.with_base_url(u);
                }
                let arc: Arc<dyn ChatProvider> = Arc::new(c);
                // Codex is cloud → ONLY non-personal reasoning roles. Never LocalPrivate.
                router = router
                    .with_provider(ModelRole::CheapWorker, arc.clone())
                    .with_provider(ModelRole::StrongReasoner, arc);
            }
            Err(e) => {
                tracing::warn!(
                    "codex_oauth selected but auth unavailable ({e}); reasoning stays noop"
                );
            }
        },
        ReasoningMode::Api => {
            if let Some(p) = cfg.cheap_worker.as_ref().and_then(build_provider) {
                router = router.with_provider(ModelRole::CheapWorker, p);
            }
            if let Some(p) = cfg.strong_reasoner.as_ref().and_then(build_provider) {
                router = router.with_provider(ModelRole::StrongReasoner, p);
            }
        }
    }

    router
}

/// Build one provider from a spec, reading its API key from the keyring/env. Returns
/// `None` if a required credential or field is missing (caller falls through to noop).
fn build_provider(spec: &ProviderSettings) -> Option<Arc<dyn ChatProvider>> {
    let kind = spec.kind.as_deref().unwrap_or("openai_compat");
    match kind {
        "openai_compat" => {
            let base_url = spec.base_url.as_deref()?;
            let model = spec.model.as_deref().unwrap_or("default");
            // Key is optional (local servers like Ollama need none).
            let key = spec.secret_key.as_deref().and_then(read_secret);
            Some(Arc::new(OpenAICompatProvider::new(
                "openai-compat",
                base_url,
                key,
                model,
            )))
        }
        "gemini" => {
            // Gemini requires a key.
            let key = spec.secret_key.as_deref().and_then(read_secret)?;
            let mut g = GeminiFlashChat::from_key(key);
            if let Some(m) = &spec.model {
                g = g.with_model(m);
            }
            Some(Arc::new(g))
        }
        "anthropic" => {
            let key = spec.secret_key.as_deref().and_then(read_secret)?;
            let model = spec
                .model
                .clone()
                .unwrap_or_else(|| crate::anthropic::DEFAULT_ANTHROPIC_MODEL.to_string());
            Some(Arc::new(AnthropicProvider::new(key, model)))
        }
        other => {
            tracing::warn!("unknown llm provider kind '{other}' — ignoring");
            None
        }
    }
}

/// Read a secret by name from the keyring, falling back to an env var of the same
/// name (mirrors `GeminiFlashChat::from_secrets_or_env`).
fn read_secret(name: &str) -> Option<String> {
    let store = altevra_secrets::SecretStore::new_keyring("altevra");
    if let Ok(Some(k)) = store.get(name) {
        return Some(k);
    }
    std::env::var(name).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn write_codex_fixture(dir: &Path) -> std::path::PathBuf {
        let p = dir.join("auth.json");
        let tok = concat!("test-", "access-", "token");
        let body = format!(
            r#"{{"tokens":{{"access_token":"{tok}","account_id":"11111111-2222-3333-4444-555555555555"}},"last_refresh":"2026-06-01T00:00:00Z"}}"#
        );
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn delegated_mode_is_all_noop() {
        let cfg = LlmConfig::default(); // reasoning_mode = delegated
        let router = build_router(&cfg);
        assert_eq!(router.resolve(ModelRole::StrongReasoner).id(), "noop");
        assert_eq!(router.resolve(ModelRole::CheapWorker).id(), "noop");
        assert_eq!(router.resolve(ModelRole::LocalPrivate).id(), "noop");
    }

    #[test]
    fn codex_mode_backs_reasoning_but_never_local_private() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_codex_fixture(dir.path());
        std::env::set_var("ALTEVRA_CODEX_AUTH_PATH", &p);

        let cfg = LlmConfig {
            reasoning_mode: ReasoningMode::CodexOauth,
            ..Default::default()
        };
        let router = build_router(&cfg);
        assert_eq!(
            router.resolve(ModelRole::StrongReasoner).id(),
            "codex-oauth"
        );
        assert_eq!(router.resolve(ModelRole::CheapWorker).id(), "codex-oauth");
        // THE headline SI-7 assertion: cloud reasoning must NOT reach local_private.
        assert_eq!(router.resolve(ModelRole::LocalPrivate).id(), "noop");

        std::env::remove_var("ALTEVRA_CODEX_AUTH_PATH");
    }

    #[test]
    fn local_private_nonlocal_spec_falls_to_noop() {
        let cfg = LlmConfig {
            local_private: Some(ProviderSettings {
                kind: Some("openai_compat".into()),
                base_url: Some("https://api.deepseek.com/v1".into()),
                model: Some("x".into()),
                secret_key: None,
            }),
            ..Default::default()
        };
        let router = build_router(&cfg);
        // factory guard #1 refuses to register a non-local provider for local_private.
        assert_eq!(router.resolve(ModelRole::LocalPrivate).id(), "noop");
    }

    #[test]
    fn local_private_localhost_spec_is_registered_and_local() {
        let cfg = LlmConfig {
            local_private: Some(ProviderSettings {
                kind: Some("openai_compat".into()),
                base_url: Some("http://localhost:11434/v1".into()),
                model: Some("qwen2.5".into()),
                secret_key: None,
            }),
            ..Default::default()
        };
        let router = build_router(&cfg);
        let p = router.resolve(ModelRole::LocalPrivate);
        assert_ne!(p.id(), "noop");
        assert!(p.is_local());
    }

    #[test]
    fn api_mode_missing_key_falls_to_noop() {
        let cfg = LlmConfig {
            reasoning_mode: ReasoningMode::Api,
            strong_reasoner: Some(ProviderSettings {
                kind: Some("gemini".into()), // gemini requires a key
                base_url: None,
                model: Some("gemini-2.0-flash".into()),
                secret_key: Some("ALTEVRA_TEST_NONEXISTENT_KEY_XYZ".into()),
            }),
            ..Default::default()
        };
        let router = build_router(&cfg);
        assert_eq!(router.resolve(ModelRole::StrongReasoner).id(), "noop");
    }
}

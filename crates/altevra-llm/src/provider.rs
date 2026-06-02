//! Model role routing + provider trait (BUILD_TASKS T5.1, RECONCILIATION R10).
//!
//! The resident runtime (P0.5+) routes by ROLE, never by concrete model. In P0
//! every role resolves to the [`NoopProvider`] so the resident contract + tests
//! run with NO network and NO keys — exactly the "just add API keys" property:
//! when Pavle adds keys, real providers replace the noop and the brain comes
//! alive without any contract change.

use crate::chat::{ChatMessage, ChatOpts};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

/// The model roles the resident modes request (V5 model routing). A mode never
/// names a concrete model; it names a role and the router resolves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelRole {
    /// Cheap, fast classification/categorization.
    CheapWorker,
    /// Deep synthesis/reasoning.
    StrongReasoner,
    /// Personal-domain ops — MUST stay local (never a US/Chinese cloud, SI-7).
    LocalPrivate,
    Embedding,
    Reranker,
    /// No model needed (pure structured work).
    None,
}

impl ModelRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            ModelRole::CheapWorker => "cheap_worker",
            ModelRole::StrongReasoner => "strong_reasoner",
            ModelRole::LocalPrivate => "local_private",
            ModelRole::Embedding => "embedding",
            ModelRole::Reranker => "reranker",
            ModelRole::None => "none",
        }
    }
}

/// A chat-completion provider. Generalizes `GeminiFlashChat`; real providers
/// (OpenAI-compat, Anthropic, local) implement this when keys are configured.
#[async_trait]
pub trait ChatProvider: Send + Sync {
    /// Stable id (e.g. "noop", "gemini-flash", "deepseek-chat").
    fn id(&self) -> &str;
    /// `true` if this provider runs locally (no data leaves the machine).
    fn is_local(&self) -> bool {
        false
    }
    async fn complete(&self, messages: &[ChatMessage], opts: &ChatOpts) -> anyhow::Result<String>;
}

/// The P0 default provider: deterministic, offline, key-free. Returns a clearly
/// marked stub so resident dry-runs and tests work without a model. Local by
/// definition (it never leaves the machine).
pub struct NoopProvider;

#[async_trait]
impl ChatProvider for NoopProvider {
    fn id(&self) -> &str {
        "noop"
    }
    fn is_local(&self) -> bool {
        true
    }
    async fn complete(&self, messages: &[ChatMessage], _opts: &ChatOpts) -> anyhow::Result<String> {
        // Deterministic, content-free stub. Echoes only the message count so a
        // test can assert it ran, never inventing model output.
        Ok(format!(
            "[noop-provider: no model configured — {} message(s) received; \
             add API keys to enable a real provider]",
            messages.len()
        ))
    }
}

/// Routes a [`ModelRole`] to a [`ChatProvider`]. With no keys configured, every
/// role maps to the noop provider. `local_private` MUST always resolve to a
/// local provider (SI-7) — the router enforces that invariant.
#[derive(Clone)]
pub struct ModelRouter {
    providers: HashMap<ModelRole, Arc<dyn ChatProvider>>,
    fallback: Arc<dyn ChatProvider>,
}

impl ModelRouter {
    /// The all-noop router (P0 default; no keys).
    pub fn noop() -> Self {
        Self {
            providers: HashMap::new(),
            fallback: Arc::new(NoopProvider),
        }
    }

    /// Register a real provider for a role (called when keys are configured).
    pub fn with_provider(mut self, role: ModelRole, provider: Arc<dyn ChatProvider>) -> Self {
        self.providers.insert(role, provider);
        self
    }

    /// Resolve a role to a provider. SI-7: `local_private` may only resolve to a
    /// local provider; a non-local registration for it is ignored in favor of
    /// the (local) fallback.
    pub fn resolve(&self, role: ModelRole) -> Arc<dyn ChatProvider> {
        if let Some(p) = self.providers.get(&role) {
            if role == ModelRole::LocalPrivate && !p.is_local() {
                return self.fallback.clone(); // enforce SI-7
            }
            return p.clone();
        }
        self.fallback.clone()
    }
}

impl Default for ModelRouter {
    fn default() -> Self {
        Self::noop()
    }
}

impl std::fmt::Debug for ModelRouter {
    // Manual: providers are trait objects (no Debug) and could hold tokens. Print
    // only the registered roles + fallback id — never provider internals.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut roles: Vec<&'static str> =
            self.providers.keys().map(|r| r.as_str()).collect();
        roles.sort_unstable();
        f.debug_struct("ModelRouter")
            .field("roles", &roles)
            .field("fallback", &self.fallback.id())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeCloud;
    #[async_trait]
    impl ChatProvider for FakeCloud {
        fn id(&self) -> &str {
            "fake-cloud"
        }
        fn is_local(&self) -> bool {
            false
        }
        async fn complete(&self, _m: &[ChatMessage], _o: &ChatOpts) -> anyhow::Result<String> {
            Ok("cloud output".into())
        }
    }

    #[tokio::test]
    async fn noop_router_resolves_everything_to_noop() {
        let r = ModelRouter::noop();
        let p = r.resolve(ModelRole::StrongReasoner);
        assert_eq!(p.id(), "noop");
        let out = p
            .complete(&[ChatMessage::user("hi")], &ChatOpts::default())
            .await
            .unwrap();
        assert!(out.contains("no model configured"));
    }

    #[tokio::test]
    async fn local_private_rejects_non_local_provider() {
        // SI-7: registering a cloud provider for local_private must NOT be used.
        let r = ModelRouter::noop().with_provider(ModelRole::LocalPrivate, Arc::new(FakeCloud));
        let p = r.resolve(ModelRole::LocalPrivate);
        assert_eq!(
            p.id(),
            "noop",
            "local_private must never resolve to a cloud provider"
        );
        assert!(p.is_local());
    }

    #[tokio::test]
    async fn registered_cloud_used_for_non_personal_role() {
        let r = ModelRouter::noop().with_provider(ModelRole::StrongReasoner, Arc::new(FakeCloud));
        assert_eq!(r.resolve(ModelRole::StrongReasoner).id(), "fake-cloud");
    }
}

//! Routes an object's embedding to the right provider by its domain's embedding role.
//! This is the embedding analogue of `altevra_llm::ModelRouter` and enforces SI-7 the
//! same way: for `LocalPrivate`, the cloud provider is STRUCTURALLY UNREACHABLE — a
//! different match arm that can only ever return the local embedder. Personal/health/
//! relationship content can never be sent to a cloud embedding API.
//!
//! The caller (e.g. the embed CLI / worker) maps `altevra_db::EmbeddingModelRole`
//! (resolved per object via `DomainPolicyRepository::embedding_role_for`) onto the
//! local [`EmbeddingRole`] here, so this crate needs no dependency on altevra-db.

use crate::embedding::AsyncEmbeddingProvider;
use std::sync::Arc;

/// Local mirror of the domain-policy embedding role (kept dep-free).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingRole {
    /// MUST embed locally (SI-7).
    LocalPrivate,
    /// May use a cloud embedder if one is configured.
    CloudOk,
}

pub struct EmbeddingRouter {
    local: Arc<dyn AsyncEmbeddingProvider>,
    cloud: Option<Arc<dyn AsyncEmbeddingProvider>>,
}

impl EmbeddingRouter {
    /// A router with both a local embedder (always present) and an optional cloud one.
    pub fn new(
        local: Arc<dyn AsyncEmbeddingProvider>,
        cloud: Option<Arc<dyn AsyncEmbeddingProvider>>,
    ) -> Self {
        Self { local, cloud }
    }

    /// Local-only router: everything embeds locally (the `embedding_mode = local` case).
    pub fn local_only(local: Arc<dyn AsyncEmbeddingProvider>) -> Self {
        Self { local, cloud: None }
    }

    /// Resolve the embedder for a role. SI-7: `LocalPrivate` NEVER reaches `cloud`.
    pub fn resolve(&self, role: EmbeddingRole) -> Arc<dyn AsyncEmbeddingProvider> {
        match role {
            // Hard guard: this arm cannot return the cloud provider.
            EmbeddingRole::LocalPrivate => self.local.clone(),
            EmbeddingRole::CloudOk => self.cloud.clone().unwrap_or_else(|| self.local.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::{Embedding, NoOpEmbedder};
    use async_trait::async_trait;

    struct FakeCloud;
    #[async_trait]
    impl AsyncEmbeddingProvider for FakeCloud {
        async fn embed(&self, _t: &str) -> anyhow::Result<Embedding> {
            Ok(Embedding {
                vector: vec![0.1, 0.2],
                model: "cloud".into(),
            })
        }
        fn dim(&self) -> usize {
            2
        }
        fn model_name(&self) -> &str {
            "cloud"
        }
    }

    #[test]
    fn local_private_never_routes_to_cloud() {
        let router = EmbeddingRouter::new(Arc::new(NoOpEmbedder::new()), Some(Arc::new(FakeCloud)));
        // Even with a cloud embedder configured, local_private resolves to local.
        assert_eq!(
            router.resolve(EmbeddingRole::LocalPrivate).model_name(),
            "noop"
        );
    }

    #[test]
    fn cloud_ok_uses_cloud_when_present() {
        let router = EmbeddingRouter::new(Arc::new(NoOpEmbedder::new()), Some(Arc::new(FakeCloud)));
        assert_eq!(router.resolve(EmbeddingRole::CloudOk).model_name(), "cloud");
    }

    #[test]
    fn cloud_ok_falls_back_to_local_when_no_cloud() {
        let router = EmbeddingRouter::local_only(Arc::new(NoOpEmbedder::new()));
        assert_eq!(router.resolve(EmbeddingRole::CloudOk).model_name(), "noop");
        assert_eq!(
            router.resolve(EmbeddingRole::LocalPrivate).model_name(),
            "noop"
        );
    }
}

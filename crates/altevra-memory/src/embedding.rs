//! Embedding provider abstraction.
//!
//! Real embedding backends (OpenAI, local ONNX, pgvector ANN, ...) come later.
//! For now the crate ships a `NoOpEmbedder` so downstream code can wire the
//! trait without depending on a concrete backend.

/// A dense vector representation of a piece of text.
#[derive(Debug, Clone, PartialEq)]
pub struct Embedding {
    pub vector: Vec<f32>,
    pub model: String,
}

/// Pluggable embedding backend. Implementations must be `Send + Sync` so they
/// can be stored behind `Arc` in long-lived components.
pub trait EmbeddingProvider: Send + Sync {
    fn embed(&self, text: &str) -> anyhow::Result<Embedding>;
    fn dim(&self) -> usize;
    fn model_name(&self) -> &str;
}

/// Placeholder provider that returns an empty vector. Useful as a default
/// when embeddings are not yet configured.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoOpEmbedder;

impl NoOpEmbedder {
    pub const MODEL_NAME: &'static str = "noop";

    pub fn new() -> Self {
        Self
    }
}

impl EmbeddingProvider for NoOpEmbedder {
    fn embed(&self, _text: &str) -> anyhow::Result<Embedding> {
        Ok(Embedding {
            vector: Vec::new(),
            model: Self::MODEL_NAME.to_string(),
        })
    }

    fn dim(&self) -> usize {
        0
    }

    fn model_name(&self) -> &str {
        Self::MODEL_NAME
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_returns_empty_vector_and_correct_model() {
        let e = NoOpEmbedder::new();
        let emb = e.embed("anything").expect("noop never fails");
        assert!(emb.vector.is_empty());
        assert_eq!(emb.model, "noop");
        assert_eq!(e.dim(), 0);
        assert_eq!(e.model_name(), "noop");
    }

    #[test]
    fn noop_is_object_safe_via_dyn_trait() {
        // Compile-time check that the trait can be used behind a dyn pointer.
        let provider: Box<dyn EmbeddingProvider> = Box::new(NoOpEmbedder::new());
        let emb = provider.embed("hello").unwrap();
        assert_eq!(emb.model, "noop");
        assert_eq!(provider.dim(), 0);
    }
}

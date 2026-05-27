//! Embedding provider abstraction.
//!
//! Real embedding backends (Gemini, OpenAI, local ONNX, ...) come via the
//! `AsyncEmbeddingProvider` trait. The sync `EmbeddingProvider` is kept for
//! existing call-sites and for the `NoOpEmbedder` test stub.

use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq)]
pub struct Embedding {
    pub vector: Vec<f32>,
    pub model: String,
}

pub trait EmbeddingProvider: Send + Sync {
    fn embed(&self, text: &str) -> anyhow::Result<Embedding>;
    fn dim(&self) -> usize;
    fn model_name(&self) -> &str;
}

/// Async variant for HTTP-backed embedders (Gemini, OpenAI, ...).
#[async_trait]
pub trait AsyncEmbeddingProvider: Send + Sync {
    async fn embed(&self, text: &str) -> anyhow::Result<Embedding>;
    async fn embed_batch(&self, texts: &[String]) -> anyhow::Result<Vec<Embedding>> {
        let mut out = Vec::with_capacity(texts.len());
        for t in texts {
            out.push(self.embed(t).await?);
        }
        Ok(out)
    }
    fn dim(&self) -> usize;
    fn model_name(&self) -> &str;
}

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

#[async_trait]
impl AsyncEmbeddingProvider for NoOpEmbedder {
    async fn embed(&self, text: &str) -> anyhow::Result<Embedding> {
        EmbeddingProvider::embed(self, text)
    }
    fn dim(&self) -> usize {
        EmbeddingProvider::dim(self)
    }
    fn model_name(&self) -> &str {
        EmbeddingProvider::model_name(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_returns_empty_vector_and_correct_model() {
        let e = NoOpEmbedder::new();
        let emb = EmbeddingProvider::embed(&e, "anything").expect("noop never fails");
        assert!(emb.vector.is_empty());
        assert_eq!(emb.model, "noop");
        assert_eq!(EmbeddingProvider::dim(&e), 0);
        assert_eq!(EmbeddingProvider::model_name(&e), "noop");
    }

    #[tokio::test]
    async fn noop_async_works() {
        let e = NoOpEmbedder::new();
        let emb = AsyncEmbeddingProvider::embed(&e, "x").await.unwrap();
        assert_eq!(emb.model, "noop");
    }

    #[test]
    fn noop_is_object_safe_via_dyn_trait() {
        let provider: Box<dyn EmbeddingProvider> = Box::new(NoOpEmbedder::new());
        let emb = provider.embed("hello").unwrap();
        assert_eq!(emb.model, "noop");
        assert_eq!(provider.dim(), 0);
    }
}

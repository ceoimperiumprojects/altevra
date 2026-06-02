//! BGE-M3 local dense embedder via `fastembed` (ONNX). Produces 1024-dim dense
//! vectors entirely on-device — no API, no data leaving the machine — so it is the
//! embedder for `local_private` content (SI-7). The model (~2GB) downloads from the
//! HF hub on first use; a long-running daemon should pre-warm it.
//!
//! Behind the `embedding` feature: it pulls onnxruntime (via `ort`). The default build
//! never compiles this (R12: core retrieval stays vector-free).

use crate::embedding::{AsyncEmbeddingProvider, Embedding};
use async_trait::async_trait;
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use std::sync::{Arc, Mutex};

pub const BGE_M3_MODEL: &str = "bge-m3";
pub const BGE_M3_DIM: usize = 1024;

/// Local BGE-M3 dense embedder. `TextEmbedding` is wrapped in a `Mutex` so the type is
/// `Sync`; embedding runs on a blocking thread (fastembed is synchronous).
pub struct Bge3Embedder {
    model: Arc<Mutex<TextEmbedding>>,
    dim: usize,
}

impl Bge3Embedder {
    /// Initialize BGE-M3 (downloads the model on first run; cached afterwards).
    pub fn new() -> anyhow::Result<Self> {
        let model = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::BGEM3).with_show_download_progress(false),
        )?;
        Ok(Self {
            model: Arc::new(Mutex::new(model)),
            dim: BGE_M3_DIM,
        })
    }
}

#[async_trait]
impl AsyncEmbeddingProvider for Bge3Embedder {
    async fn embed(&self, text: &str) -> anyhow::Result<Embedding> {
        let model = self.model.clone();
        let text = text.to_string();
        let vector = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<f32>> {
            let mut model = model
                .lock()
                .map_err(|_| anyhow::anyhow!("bge embedder mutex poisoned"))?;
            let mut out = model.embed(vec![text], None)?;
            Ok(out.pop().unwrap_or_default())
        })
        .await??;
        Ok(Embedding {
            vector,
            model: BGE_M3_MODEL.to_string(),
        })
    }

    async fn embed_batch(&self, texts: &[String]) -> anyhow::Result<Vec<Embedding>> {
        let model = self.model.clone();
        let texts = texts.to_vec();
        let vectors = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<Vec<f32>>> {
            let mut model = model
                .lock()
                .map_err(|_| anyhow::anyhow!("bge embedder mutex poisoned"))?;
            model.embed(texts, None)
        })
        .await??;
        Ok(vectors
            .into_iter()
            .map(|v| Embedding {
                vector: v,
                model: BGE_M3_MODEL.to_string(),
            })
            .collect())
    }

    fn dim(&self) -> usize {
        self.dim
    }
    fn model_name(&self) -> &str {
        BGE_M3_MODEL
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The real model download + inference is heavy; gate behind --ignored so CI and
    // the default test run never touch the network or build a 2GB model.
    #[tokio::test]
    #[ignore = "downloads BGE-M3 (~2GB) and runs ONNX inference; run manually"]
    async fn bge_embeds_with_correct_dim() {
        let e = Bge3Embedder::new().expect("init bge-m3");
        assert_eq!(e.model_name(), "bge-m3");
        let emb = e.embed("koliko košta licenca?").await.expect("embed");
        assert_eq!(emb.vector.len(), BGE_M3_DIM);
        assert_eq!(emb.model, "bge-m3");
    }
}

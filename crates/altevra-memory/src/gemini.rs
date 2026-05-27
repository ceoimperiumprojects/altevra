//! Gemini embedding provider — uses Google's text-embedding-004 model.
//!
//! Free tier: 1500 requests per minute. 768-dim vectors.
//! API key is read from the Altevra secret store under the configured key name
//! (default: GEMINI_API_KEY), with env var fallback.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

use crate::embedding::{AsyncEmbeddingProvider, Embedding};

pub const GEMINI_MODEL: &str = "text-embedding-004";
pub const GEMINI_DIM: usize = 768;
const ENDPOINT: &str =
    "https://generativelanguage.googleapis.com/v1beta/models/text-embedding-004:embedContent";

#[derive(Debug, Serialize)]
struct EmbedRequest<'a> {
    model: String,
    content: EmbedContent<'a>,
}

#[derive(Debug, Serialize)]
struct EmbedContent<'a> {
    parts: Vec<EmbedPart<'a>>,
}

#[derive(Debug, Serialize)]
struct EmbedPart<'a> {
    text: &'a str,
}

#[derive(Debug, Deserialize)]
struct EmbedResponse {
    embedding: EmbedValues,
}

#[derive(Debug, Deserialize)]
struct EmbedValues {
    values: Vec<f32>,
}

#[derive(Debug, Deserialize)]
struct ApiError {
    error: ApiErrorBody,
}

#[derive(Debug, Deserialize)]
struct ApiErrorBody {
    message: String,
    code: Option<i32>,
}

pub struct GeminiEmbedder {
    api_key: String,
    client: reqwest::Client,
    cache: Mutex<std::collections::HashMap<String, Vec<f32>>>,
}

impl GeminiEmbedder {
    /// Build from explicit key.
    pub fn from_key(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            cache: Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Resolve key from Altevra secret store first, then env var fallback.
    pub fn from_secrets_or_env() -> anyhow::Result<Self> {
        let secret_key = std::env::var("ALTEVRA_GEMINI_SECRET_KEY")
            .unwrap_or_else(|_| "GEMINI_API_KEY".to_string());
        // Try keyring first (default backend)
        let keyring_store = altevra_secrets::SecretStore::new_keyring("altevra");
        if let Ok(Some(key)) = keyring_store.get(&secret_key) {
            return Ok(Self::from_key(key));
        }
        // Then env var
        if let Ok(key) = std::env::var(&secret_key) {
            return Ok(Self::from_key(key));
        }
        anyhow::bail!(
            "Gemini API key not found in keyring under '{secret_key}' or env var. \
             Run: altevra secrets set {secret_key}",
        );
    }

    fn cache_get(&self, text: &str) -> Option<Vec<f32>> {
        self.cache.lock().ok()?.get(text).cloned()
    }

    fn cache_put(&self, text: &str, v: Vec<f32>) {
        if let Ok(mut g) = self.cache.lock() {
            g.insert(text.to_string(), v);
        }
    }
}

#[async_trait]
impl AsyncEmbeddingProvider for GeminiEmbedder {
    async fn embed(&self, text: &str) -> anyhow::Result<Embedding> {
        if text.trim().is_empty() {
            return Ok(Embedding {
                vector: vec![0.0; GEMINI_DIM],
                model: GEMINI_MODEL.to_string(),
            });
        }

        if let Some(cached) = self.cache_get(text) {
            return Ok(Embedding {
                vector: cached,
                model: GEMINI_MODEL.to_string(),
            });
        }

        let url = format!("{ENDPOINT}?key={}", self.api_key);
        let body = EmbedRequest {
            model: format!("models/{GEMINI_MODEL}"),
            content: EmbedContent {
                parts: vec![EmbedPart { text }],
            },
        };
        let resp = self.client.post(&url).json(&body).send().await?;
        let status = resp.status();
        let raw = resp.text().await?;
        if !status.is_success() {
            let msg = serde_json::from_str::<ApiError>(&raw)
                .map(|e| e.error.message)
                .unwrap_or_else(|_| raw.clone());
            anyhow::bail!("Gemini embed failed ({status}): {msg}");
        }
        let parsed: EmbedResponse = serde_json::from_str(&raw)?;
        self.cache_put(text, parsed.embedding.values.clone());

        Ok(Embedding {
            vector: parsed.embedding.values,
            model: GEMINI_MODEL.to_string(),
        })
    }

    fn dim(&self) -> usize {
        GEMINI_DIM
    }
    fn model_name(&self) -> &str {
        GEMINI_MODEL
    }
}

/// Cosine similarity between two vectors. Returns 0.0 if either is empty or
/// dimensions mismatch.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_returns_zero_vector() {
        // We can't test the actual API without credentials, but we can ensure
        // the zero-text path stays cheap.
        let e = GeminiEmbedder::from_key("dummy");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let emb = rt.block_on(e.embed("   ")).unwrap();
        assert_eq!(emb.vector.len(), GEMINI_DIM);
        assert!(emb.vector.iter().all(|x| *x == 0.0));
    }

    #[test]
    fn cache_get_returns_none_for_unknown() {
        let e = GeminiEmbedder::from_key("dummy");
        assert!(e.cache_get("nothing").is_none());
    }

    #[test]
    fn cosine_basics() {
        assert_eq!(cosine(&[1.0, 0.0], &[1.0, 0.0]), 1.0);
        assert_eq!(cosine(&[1.0, 0.0], &[0.0, 1.0]), 0.0);
        let v = cosine(&[1.0, 2.0, 3.0], &[1.0, 2.0, 3.0]);
        assert!((v - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_handles_empty_or_mismatched() {
        assert_eq!(cosine(&[], &[1.0]), 0.0);
        assert_eq!(cosine(&[1.0, 2.0], &[1.0]), 0.0);
    }

    #[test]
    fn dim_is_768() {
        let e = GeminiEmbedder::from_key("dummy");
        assert_eq!(e.dim(), GEMINI_DIM);
        assert_eq!(e.model_name(), GEMINI_MODEL);
    }
}

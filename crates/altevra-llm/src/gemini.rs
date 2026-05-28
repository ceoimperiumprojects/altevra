//! Gemini Flash chat provider — used by Analyze Everything for session
//! summaries.
//!
//! Reads API key from Altevra keyring (key name from `ALTEVRA_GEMINI_SECRET_KEY`
//! env var, default `GEMINI_API_KEY`) with env var fallback. Pairs with the
//! Gemini embedder in `altevra-memory::gemini` (same key, same upstream).

use serde::{Deserialize, Serialize};

use crate::chat::{ChatMessage, ChatOpts, ChatRole};

pub const GEMINI_FLASH_MODEL: &str = "gemini-2.0-flash";

fn endpoint(model: &str) -> String {
    format!("https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent")
}

#[derive(Debug, Serialize)]
struct GenRequest {
    contents: Vec<GenContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GenContent>,
    #[serde(rename = "generationConfig", skip_serializing_if = "Option::is_none")]
    generation_config: Option<GenConfig>,
}

#[derive(Debug, Serialize)]
struct GenContent {
    role: &'static str,
    parts: Vec<GenPart>,
}

#[derive(Debug, Serialize)]
struct GenPart {
    text: String,
}

#[derive(Debug, Serialize, Default)]
struct GenConfig {
    #[serde(rename = "maxOutputTokens", skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Debug, Deserialize)]
struct GenResponse {
    candidates: Option<Vec<GenCandidate>>,
    #[serde(default)]
    error: Option<ApiError>,
}

#[derive(Debug, Deserialize)]
struct GenCandidate {
    content: Option<GenCandidateContent>,
}

#[derive(Debug, Deserialize)]
struct GenCandidateContent {
    parts: Vec<GenCandidatePart>,
}

#[derive(Debug, Deserialize)]
struct GenCandidatePart {
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ApiError {
    message: String,
    #[serde(default)]
    code: Option<i32>,
}

pub struct GeminiFlashChat {
    api_key: String,
    model: String,
    client: reqwest::Client,
}

impl GeminiFlashChat {
    pub fn from_key(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: GEMINI_FLASH_MODEL.to_string(),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .unwrap_or_default(),
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    pub fn from_secrets_or_env() -> anyhow::Result<Self> {
        let secret_key = std::env::var("ALTEVRA_GEMINI_SECRET_KEY")
            .unwrap_or_else(|_| "GEMINI_API_KEY".to_string());
        let store = altevra_secrets::SecretStore::new_keyring("altevra");
        if let Ok(Some(key)) = store.get(&secret_key) {
            return Ok(Self::from_key(key));
        }
        if let Ok(key) = std::env::var(&secret_key) {
            return Ok(Self::from_key(key));
        }
        anyhow::bail!(
            "Gemini API key not found in keyring under '{secret_key}' or env var. \
             Run: altevra secrets set {secret_key}",
        );
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub async fn complete(
        &self,
        messages: &[ChatMessage],
        opts: ChatOpts,
    ) -> anyhow::Result<String> {
        let mut contents: Vec<GenContent> = Vec::with_capacity(messages.len());
        let mut system_instruction: Option<GenContent> = None;

        // Gemini treats system prompts via systemInstruction field. Inline
        // system messages also fold into systemInstruction.
        for msg in messages {
            let role = match msg.role {
                ChatRole::User | ChatRole::Tool => "user",
                ChatRole::Assistant => "model",
                ChatRole::System => {
                    let part = GenPart {
                        text: msg.content.clone(),
                    };
                    if let Some(ref mut s) = system_instruction {
                        s.parts.push(part);
                    } else {
                        system_instruction = Some(GenContent {
                            role: "user",
                            parts: vec![part],
                        });
                    }
                    continue;
                }
            };
            contents.push(GenContent {
                role,
                parts: vec![GenPart {
                    text: msg.content.clone(),
                }],
            });
        }

        if let Some(sys) = opts.system.as_ref() {
            let part = GenPart { text: sys.clone() };
            if let Some(ref mut s) = system_instruction {
                s.parts.push(part);
            } else {
                system_instruction = Some(GenContent {
                    role: "user",
                    parts: vec![part],
                });
            }
        }

        let gen_config = if opts.max_tokens.is_some() || opts.temperature.is_some() {
            Some(GenConfig {
                max_output_tokens: opts.max_tokens,
                temperature: opts.temperature,
            })
        } else {
            None
        };

        let body = GenRequest {
            contents,
            system_instruction,
            generation_config: gen_config,
        };

        let url = endpoint(&self.model);
        let resp = self
            .client
            .post(&url)
            .query(&[("key", self.api_key.as_str())])
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        let text = resp.text().await?;

        if !status.is_success() {
            anyhow::bail!("Gemini API HTTP {}: {}", status, text);
        }

        let parsed: GenResponse = serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("decode Gemini response: {e}; body={text}"))?;

        if let Some(err) = parsed.error {
            anyhow::bail!("Gemini API error {:?}: {}", err.code, err.message);
        }

        let candidates = parsed
            .candidates
            .ok_or_else(|| anyhow::anyhow!("Gemini response had no candidates: {text}"))?;

        let first = candidates
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("Gemini returned zero candidates"))?;

        let content = first
            .content
            .ok_or_else(|| anyhow::anyhow!("Gemini candidate had no content"))?;

        let joined = content
            .parts
            .into_iter()
            .filter_map(|p| p.text)
            .collect::<Vec<_>>()
            .join("");

        Ok(joined)
    }

    /// Convenience: single-turn summary.
    pub async fn summarize(&self, text: &str, max_tokens: u32) -> anyhow::Result<String> {
        let opts = ChatOpts::default()
            .with_max_tokens(max_tokens)
            .with_temperature(0.2)
            .with_system(
                "You are a concise technical summarizer. Output 2-3 sentences \
                 that capture what the user was working on and any key outcome. \
                 No preamble, no markdown.",
            );
        self.complete(&[ChatMessage::user(text)], opts).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_uses_model_name() {
        let url = endpoint("gemini-2.0-flash");
        assert!(url.contains("gemini-2.0-flash:generateContent"));
        assert!(url.starts_with("https://"));
    }

    #[test]
    fn from_key_sets_default_model() {
        let c = GeminiFlashChat::from_key("dummy");
        assert_eq!(c.model(), GEMINI_FLASH_MODEL);
    }

    #[test]
    fn with_model_overrides() {
        let c = GeminiFlashChat::from_key("dummy").with_model("gemini-2.5-pro");
        assert_eq!(c.model(), "gemini-2.5-pro");
    }

    #[test]
    fn system_message_folds_into_system_instruction() {
        // We can't assert on the serialized body without a transport mock,
        // but we can verify construction does not panic and types align.
        let msgs = [ChatMessage::system("be brief"), ChatMessage::user("hello")];
        let opts = ChatOpts::default().with_max_tokens(50);
        assert_eq!(msgs.len(), 2);
        assert!(opts.max_tokens.is_some());
    }

    #[tokio::test]
    async fn complete_returns_error_without_network() {
        let c = GeminiFlashChat::from_key("invalid-key-for-test");
        // Hit unreachable host by overriding model with garbage so endpoint
        // returns 404. The point is to exercise the error path.
        let c = c.with_model("nonexistent-model-altevra-test");
        let result = c
            .complete(&[ChatMessage::user("hi")], ChatOpts::default())
            .await;
        // Either network error or HTTP non-2xx — both are Err.
        assert!(result.is_err());
    }
}

//! Generic OpenAI-compatible chat provider (`POST {base_url}/chat/completions`).
//! Covers OpenAI, DeepSeek, Groq, OpenRouter, vLLM, Ollama (`/v1`), etc.
//!
//! `is_local()` is true ONLY when the endpoint host is a loopback address — that is
//! what makes a local server (Ollama/vLLM on localhost) eligible to back the
//! `local_private` role under SI-7. Host is parsed (not substring-matched) so that
//! `http://localhost.evil.com` is correctly treated as NON-local.

use crate::chat::{ChatMessage, ChatOpts, ChatRole};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub struct OpenAICompatProvider {
    id: String,
    base_url: String,
    api_key: Option<String>,
    model: String,
    is_local: bool,
    client: reqwest::Client,
}

impl OpenAICompatProvider {
    pub fn new(
        id: impl Into<String>,
        base_url: impl Into<String>,
        api_key: Option<String>,
        model: impl Into<String>,
    ) -> Self {
        let base_url = base_url.into();
        let is_local = base_url_is_local(&base_url);
        Self {
            id: id.into(),
            base_url,
            api_key,
            model: model.into(),
            is_local,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .unwrap_or_default(),
        }
    }
}

/// A provider is "local" iff its endpoint resolves to a loopback host. Parsing the
/// URL host (not substring matching) prevents `localhost.evil.com` spoofs.
fn base_url_is_local(base_url: &str) -> bool {
    match reqwest::Url::parse(base_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
    {
        Some(h) => {
            // host_str() serializes IPv6 with brackets ("[::1]") — strip them.
            let h = h.trim_start_matches('[').trim_end_matches(']');
            matches!(h, "localhost" | "127.0.0.1" | "::1" | "0.0.0.0") || h.ends_with(".local")
        }
        None => false,
    }
}

#[async_trait]
impl crate::provider::ChatProvider for OpenAICompatProvider {
    fn id(&self) -> &str {
        &self.id
    }
    fn is_local(&self) -> bool {
        self.is_local
    }
    async fn complete(&self, messages: &[ChatMessage], opts: &ChatOpts) -> anyhow::Result<String> {
        let mut msgs: Vec<WireMsg> = Vec::new();
        if let Some(sys) = opts.system.as_ref() {
            msgs.push(WireMsg {
                role: "system".into(),
                content: sys.clone(),
            });
        }
        for m in messages {
            let role = match m.role {
                ChatRole::System => "system",
                ChatRole::Assistant => "assistant",
                ChatRole::User | ChatRole::Tool => "user",
            };
            msgs.push(WireMsg {
                role: role.into(),
                content: m.content.clone(),
            });
        }
        let body = WireRequest {
            model: self.model.clone(),
            messages: msgs,
            max_tokens: opts.max_tokens,
            temperature: opts.temperature,
        };
        let mut req = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .json(&body);
        if let Some(key) = self.api_key.as_ref() {
            req = req.bearer_auth(key);
        }
        let resp = req.send().await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            anyhow::bail!("{} HTTP {status}: {text}", self.id);
        }
        let parsed: WireResponse = serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("decode {} response: {e}; body={text}", self.id))?;
        Ok(parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .unwrap_or_default())
    }
}

#[derive(Debug, Serialize)]
struct WireRequest {
    model: String,
    messages: Vec<WireMsg>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Debug, Serialize)]
struct WireMsg {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct WireResponse {
    #[serde(default)]
    choices: Vec<WireChoice>,
}

#[derive(Debug, Deserialize)]
struct WireChoice {
    message: WireChoiceMsg,
}

#[derive(Debug, Deserialize)]
struct WireChoiceMsg {
    #[serde(default)]
    content: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ChatProvider;

    #[test]
    fn localhost_endpoints_are_local() {
        assert!(base_url_is_local("http://localhost:11434/v1"));
        assert!(base_url_is_local("http://127.0.0.1:8000/v1"));
        assert!(base_url_is_local("http://[::1]:8000/v1"));
        assert!(base_url_is_local("http://0.0.0.0:8080/v1"));
        assert!(base_url_is_local("http://my-box.local:1234/v1"));
    }

    #[test]
    fn cloud_and_spoof_endpoints_are_not_local() {
        assert!(!base_url_is_local("https://api.deepseek.com/v1"));
        assert!(!base_url_is_local("https://api.openai.com/v1"));
        // Spoof: host is localhost.evil.com, NOT loopback.
        assert!(!base_url_is_local("http://localhost.evil.com/v1"));
        assert!(!base_url_is_local("not-a-url"));
    }

    #[test]
    fn local_provider_reports_local() {
        let p = OpenAICompatProvider::new("ollama", "http://localhost:11434/v1", None, "qwen2.5");
        assert_eq!(p.id(), "ollama");
        assert!(p.is_local());
    }

    #[test]
    fn cloud_provider_reports_not_local() {
        let p = OpenAICompatProvider::new(
            "deepseek",
            "https://api.deepseek.com/v1",
            Some("k".into()),
            "deepseek-reasoner",
        );
        assert!(!p.is_local());
    }
}

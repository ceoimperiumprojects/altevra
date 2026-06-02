//! Anthropic Messages API chat provider (`POST https://api.anthropic.com/v1/messages`).
//!
//! Anthropic differs from OpenAI: `system` is a TOP-LEVEL field (not a message),
//! `max_tokens` is REQUIRED, and the response text lives in a `content[]` array of
//! typed blocks. Cloud provider → `is_local()=false` (never backs local_private).

use crate::chat::{ChatMessage, ChatOpts, ChatRole};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

const ANTHROPIC_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 1024;
pub const DEFAULT_ANTHROPIC_MODEL: &str = "claude-3-5-haiku-latest";

pub struct AnthropicProvider {
    api_key: String,
    model: String,
    client: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: model.into(),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .unwrap_or_default(),
        }
    }

    fn build_body(&self, messages: &[ChatMessage], opts: &ChatOpts) -> MessagesRequest {
        let mut system = String::new();
        let mut msgs: Vec<WireMsg> = Vec::new();
        for m in messages {
            match m.role {
                ChatRole::System => push(&mut system, &m.content),
                ChatRole::Assistant => msgs.push(WireMsg::new("assistant", &m.content)),
                ChatRole::User | ChatRole::Tool => msgs.push(WireMsg::new("user", &m.content)),
            }
        }
        if let Some(sys) = opts.system.as_ref() {
            push(&mut system, sys);
        }
        MessagesRequest {
            model: self.model.clone(),
            max_tokens: opts.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            system: if system.is_empty() { None } else { Some(system) },
            messages: msgs,
            temperature: opts.temperature,
        }
    }
}

fn push(buf: &mut String, s: &str) {
    if !buf.is_empty() {
        buf.push_str("\n\n");
    }
    buf.push_str(s);
}

#[async_trait]
impl crate::provider::ChatProvider for AnthropicProvider {
    fn id(&self) -> &str {
        "anthropic"
    }
    fn is_local(&self) -> bool {
        false
    }
    async fn complete(&self, messages: &[ChatMessage], opts: &ChatOpts) -> anyhow::Result<String> {
        let body = self.build_body(messages, opts);
        let resp = self
            .client
            .post(ANTHROPIC_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            anyhow::bail!("Anthropic HTTP {status}: {text}");
        }
        let parsed: MessagesResponse = serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("decode Anthropic response: {e}; body={text}"))?;
        Ok(parsed
            .content
            .into_iter()
            .filter(|b| b.kind == "text")
            .filter_map(|b| b.text)
            .collect::<Vec<_>>()
            .join(""))
    }
}

#[derive(Debug, Serialize)]
struct MessagesRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<WireMsg>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Debug, Serialize)]
struct WireMsg {
    role: String,
    content: String,
}

impl WireMsg {
    fn new(role: &str, content: &str) -> Self {
        Self {
            role: role.to_string(),
            content: content.to_string(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct MessagesResponse {
    #[serde(default)]
    content: Vec<ContentBlock>,
}

#[derive(Debug, Deserialize)]
struct ContentBlock {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    text: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ChatProvider;

    #[test]
    fn id_and_not_local() {
        let p = AnthropicProvider::new("k", DEFAULT_ANTHROPIC_MODEL);
        assert_eq!(p.id(), "anthropic");
        assert!(!p.is_local());
    }

    #[test]
    fn system_is_top_level_and_max_tokens_defaulted() {
        let p = AnthropicProvider::new("k", "claude-3-5-haiku-latest");
        let msgs = [ChatMessage::system("be terse"), ChatMessage::user("hi")];
        let body = p.build_body(&msgs, &ChatOpts::default());
        let v = serde_json::to_value(&body).unwrap();
        assert_eq!(v["system"], "be terse");
        assert_eq!(v["max_tokens"], DEFAULT_MAX_TOKENS);
        // system must NOT appear as a message; only the user turn is in messages.
        assert_eq!(v["messages"].as_array().unwrap().len(), 1);
        assert_eq!(v["messages"][0]["role"], "user");
    }

    #[test]
    fn response_joins_text_blocks() {
        let json = r#"{"content":[{"type":"text","text":"a"},{"type":"thinking","text":"ignore"},{"type":"text","text":"b"}]}"#;
        let parsed: MessagesResponse = serde_json::from_str(json).unwrap();
        let joined: String = parsed
            .content
            .into_iter()
            .filter(|b| b.kind == "text")
            .filter_map(|b| b.text)
            .collect();
        assert_eq!(joined, "ab");
    }
}

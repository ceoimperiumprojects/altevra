//! Chat message types shared across providers.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::System,
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::User,
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::Assistant,
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ChatOpts {
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub system: Option<String>,
}

impl ChatOpts {
    pub fn with_max_tokens(mut self, n: u32) -> Self {
        self.max_tokens = Some(n);
        self
    }

    pub fn with_temperature(mut self, t: f32) -> Self {
        self.temperature = Some(t);
        self
    }

    pub fn with_system(mut self, s: impl Into<String>) -> Self {
        self.system = Some(s.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_constructors_work() {
        assert_eq!(ChatMessage::user("hi").role, ChatRole::User);
        assert_eq!(ChatMessage::system("sys").role, ChatRole::System);
        assert_eq!(ChatMessage::assistant("hi").role, ChatRole::Assistant);
    }

    #[test]
    fn opts_builder_chains() {
        let o = ChatOpts::default()
            .with_max_tokens(100)
            .with_temperature(0.7)
            .with_system("be brief");
        assert_eq!(o.max_tokens, Some(100));
        assert_eq!(o.temperature, Some(0.7));
        assert_eq!(o.system.as_deref(), Some("be brief"));
    }

    #[test]
    fn role_serializes_lowercase() {
        let json = serde_json::to_string(&ChatRole::User).unwrap();
        assert_eq!(json, "\"user\"");
    }
}

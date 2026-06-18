//! altevra-llm — multi-provider LLM abstraction.
//!
//! Routes by ROLE (cheap_worker / strong_reasoner / local_private / …), never by a
//! concrete model. Real providers:
//!
//! - [`gemini::GeminiFlashChat`] — Google Gemini (native).
//! - [`codex_oauth::CodexOAuthProvider`] — ChatGPT (GPT-5.5) via `~/.codex/auth.json`.
//! - [`openai_compat::OpenAICompatProvider`] — any OpenAI-compatible endpoint (OpenAI, DeepSeek, Groq, OpenRouter, Ollama, vLLM, …); local when the host is loopback.
//! - [`anthropic::AnthropicProvider`] — Anthropic Messages API.
//!
//! With no keys, every role resolves to [`provider::NoopProvider`] (delegated mode):
//! the connected tool does the reasoning over MCP. SI-7: `local_private` only ever
//! resolves to a LOCAL provider — the router enforces it.

pub mod anthropic;
pub mod chat;
pub mod claude_cli;
pub mod codex_oauth;
pub mod factory;
pub mod gemini;
pub mod openai_compat;
pub mod provider;
pub mod rate_limit;

pub use anthropic::AnthropicProvider;
pub use chat::{ChatMessage, ChatOpts, ChatRole};
pub use claude_cli::ClaudeCliProvider;
pub use codex_oauth::{CodexOAuthProvider, CodexWire};
pub use factory::build_router;
pub use gemini::GeminiFlashChat;
pub use openai_compat::OpenAICompatProvider;
pub use provider::{ChatProvider, ModelRole, ModelRouter, NoopProvider};
pub use rate_limit::RateLimiter;

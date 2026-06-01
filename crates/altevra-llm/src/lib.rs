//! altevra-llm — multi-provider LLM abstraction.
//!
//! v0.3.8 first cut: Gemini Flash chat provider used by Analyze Everything for
//! automatic session summaries. v0.3.9 generalizes into full `ChatProvider` and
//! `EmbeddingProvider` traits with native (Gemini, OpenAI, Anthropic, Voyage)
//! and OpenAI-compatible (DeepSeek, Qwen, Moonshot, Zhipu, MiniMax, Baichuan,
//! Yi, Stepfun, Groq, Together, OpenRouter, Mistral, Cohere, Ollama, vLLM,
//! Custom) adapters.

pub mod chat;
pub mod gemini;
pub mod provider;
pub mod rate_limit;

pub use chat::{ChatMessage, ChatOpts, ChatRole};
pub use gemini::GeminiFlashChat;
pub use provider::{ChatProvider, ModelRole, ModelRouter, NoopProvider};
pub use rate_limit::RateLimiter;

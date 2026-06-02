//! Codex OAuth chat provider — uses Pavle's ChatGPT (GPT-5.5) via `~/.codex/auth.json`,
//! the same backend Hermes uses (`provider: openai-codex`,
//! `base_url: https://chatgpt.com/backend-api/codex`). This lets Altevra's brain
//! reason server-side with no per-token API cost, on the existing ChatGPT Plus seat.
//!
//! Two wires:
//!   * `Responses` (default) — talks the OpenAI Responses API directly to the
//!     ChatGPT backend (`.../codex/responses`), mapping chat messages → input items.
//!   * `ChatCompletions` — when `with_base_url` points at an OpenAI-compatible
//!     wrapper (e.g. a self-hosted Cloudflare Worker), uses classic
//!     `/chat/completions`. Escape hatch if the backend headers change.
//!
//! SI-7: this provider is ALWAYS cloud (`is_local()=false`) → it must NEVER back the
//! `local_private` role. The factory + `ModelRouter::resolve` enforce that.
//!
//! The OAuth token (~30-day lifetime) is read but never persisted by Altevra. On a
//! 401/403 the error tells Pavle exactly what to do: `codex login`.

use crate::chat::{ChatMessage, ChatOpts, ChatRole};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const DEFAULT_CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
pub const DEFAULT_CODEX_MODEL: &str = "gpt-5.5";

/// Which request/response shape to speak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexWire {
    /// OpenAI Responses API (direct ChatGPT backend).
    Responses,
    /// Classic /chat/completions (OpenAI-compatible wrapper).
    ChatCompletions,
}

#[derive(Debug, Deserialize)]
struct CodexAuth {
    tokens: CodexTokens,
    #[serde(default)]
    last_refresh: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CodexTokens {
    access_token: String,
    account_id: String,
}

/// Path to `~/.codex/auth.json`, overridable via `ALTEVRA_CODEX_AUTH_PATH` (used by tests).
fn default_auth_path() -> PathBuf {
    if let Ok(p) = std::env::var("ALTEVRA_CODEX_AUTH_PATH") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    PathBuf::from(home).join(".codex/auth.json")
}

pub struct CodexOAuthProvider {
    access_token: String,
    account_id: String,
    base_url: String,
    model: String,
    wire: CodexWire,
    client: reqwest::Client,
}

impl CodexOAuthProvider {
    /// Construct from an explicit auth file (no network — just reads + parses).
    pub fn from_auth_file(path: &Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path).map_err(|e| {
            anyhow::anyhow!(
                "cannot read Codex auth at {}: {e}. Run `codex login` first.",
                path.display()
            )
        })?;
        let auth: CodexAuth = serde_json::from_str(&raw)
            .map_err(|e| anyhow::anyhow!("Codex auth.json malformed: {e}"))?;

        if let Some(ref ts) = auth.last_refresh {
            if token_older_than_days(ts, 28) {
                tracing::warn!(
                    "Codex token last refreshed {ts} (>28d ago); if calls 401, run `codex login`"
                );
            }
        }

        Ok(Self {
            access_token: auth.tokens.access_token,
            account_id: auth.tokens.account_id,
            base_url: DEFAULT_CODEX_BASE_URL.to_string(),
            model: DEFAULT_CODEX_MODEL.to_string(),
            wire: CodexWire::Responses,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .unwrap_or_default(),
        })
    }

    /// Construct from the default `~/.codex/auth.json`.
    pub fn from_default_auth() -> anyhow::Result<Self> {
        Self::from_auth_file(&default_auth_path())
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Override the endpoint. If it points at a non-Codex OpenAI-compatible wrapper,
    /// switch to the `/chat/completions` wire automatically.
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        let url = url.into();
        self.wire = if url.contains("backend-api/codex") {
            CodexWire::Responses
        } else {
            CodexWire::ChatCompletions
        };
        self.base_url = url;
        self
    }

    pub fn wire(&self) -> CodexWire {
        self.wire
    }

    fn build_responses_body(&self, messages: &[ChatMessage], opts: &ChatOpts) -> ResponsesRequest {
        let mut instructions = String::new();
        let mut input: Vec<InputItem> = Vec::new();
        for m in messages {
            match m.role {
                ChatRole::System => push_instruction(&mut instructions, &m.content),
                ChatRole::Assistant => input.push(InputItem::new("assistant", &m.content)),
                ChatRole::User | ChatRole::Tool => input.push(InputItem::new("user", &m.content)),
            }
        }
        if let Some(sys) = opts.system.as_ref() {
            push_instruction(&mut instructions, sys);
        }
        ResponsesRequest {
            model: self.model.clone(),
            // The ChatGPT codex Responses backend REQUIRES a non-null `instructions`
            // field (it 400s on "Instructions are required"), so always send one —
            // a minimal default when no system prompt was provided.
            instructions: Some(if instructions.is_empty() {
                "You are a helpful assistant.".to_string()
            } else {
                instructions
            }),
            input,
            // The codex backend rejects max_output_tokens/temperature; omit them.
            max_output_tokens: None,
            temperature: None,
            // It requires streaming + no server-side storage.
            stream: true,
            store: false,
        }
    }

    fn build_chat_body(&self, messages: &[ChatMessage], opts: &ChatOpts) -> ChatRequest {
        let mut msgs: Vec<ChatMsg> = Vec::new();
        if let Some(sys) = opts.system.as_ref() {
            msgs.push(ChatMsg {
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
            msgs.push(ChatMsg {
                role: role.into(),
                content: m.content.clone(),
            });
        }
        ChatRequest {
            model: self.model.clone(),
            messages: msgs,
            max_tokens: opts.max_tokens,
            temperature: opts.temperature,
        }
    }
}

/// Map an HTTP error status + body into an actionable error. Pure so it is testable
/// without a transport.
fn map_http_error(status: reqwest::StatusCode, body: &str) -> anyhow::Error {
    if status.as_u16() == 401 || status.as_u16() == 403 {
        anyhow::anyhow!(
            "Codex OAuth token rejected (HTTP {status}). The ~/.codex/auth.json token \
             likely expired (~30-day lifetime). Re-authenticate with: codex login"
        )
    } else {
        anyhow::anyhow!("Codex backend HTTP {status}: {body}")
    }
}

fn push_instruction(buf: &mut String, s: &str) {
    if !buf.is_empty() {
        buf.push_str("\n\n");
    }
    buf.push_str(s);
}

/// Returns true if the RFC3339 timestamp is more than `days` before now. On parse
/// failure, returns false (don't warn on garbage).
fn token_older_than_days(ts: &str, days: i64) -> bool {
    match chrono::DateTime::parse_from_rfc3339(ts) {
        Ok(t) => (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_days() > days,
        Err(_) => false,
    }
}

/// Consume the SSE (`text/event-stream`) Responses stream and return the assembled
/// assistant text. Accumulates `response.output_text.delta` events; prefers the final
/// `response.completed` output_text if present. Buffers BYTES (not lossy per-chunk
/// strings) so multi-byte UTF-8 split across chunks is never corrupted.
async fn read_responses_stream(resp: reqwest::Response) -> anyhow::Result<String> {
    use futures::StreamExt;
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    let mut deltas = String::new();
    let mut final_text: Option<String> = None;

    while let Some(chunk) = stream.next().await {
        buf.extend_from_slice(&chunk?);
        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let line_bytes: Vec<u8> = buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line_bytes);
            let line = line.trim();
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            let Ok(v) = serde_json::from_str::<serde_json::Value>(data) else {
                continue;
            };
            match v.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                "response.output_text.delta" => {
                    if let Some(d) = v.get("delta").and_then(|d| d.as_str()) {
                        deltas.push_str(d);
                    }
                }
                "response.completed" => {
                    if let Some(t) = v.pointer("/response/output_text").and_then(|t| t.as_str()) {
                        final_text = Some(t.to_string());
                    }
                }
                _ => {}
            }
        }
    }
    Ok(final_text.filter(|s| !s.is_empty()).unwrap_or(deltas))
}

#[async_trait]
impl crate::provider::ChatProvider for CodexOAuthProvider {
    fn id(&self) -> &str {
        "codex-oauth"
    }
    fn is_local(&self) -> bool {
        false // ALWAYS cloud — never backs local_private (SI-7).
    }
    async fn complete(&self, messages: &[ChatMessage], opts: &ChatOpts) -> anyhow::Result<String> {
        match self.wire {
            CodexWire::Responses => {
                let body = self.build_responses_body(messages, opts);
                let resp = self
                    .client
                    .post(format!("{}/responses", self.base_url))
                    .bearer_auth(&self.access_token)
                    .header("chatgpt-account-id", &self.account_id)
                    .header("OpenAI-Beta", "responses=experimental")
                    .header("originator", "codex_cli_rs")
                    .header("Accept", "text/event-stream")
                    .json(&body)
                    .send()
                    .await?;
                let status = resp.status();
                if !status.is_success() {
                    let text = resp.text().await?;
                    return Err(map_http_error(status, &text));
                }
                // The codex backend only streams (SSE). Accumulate the output_text deltas.
                read_responses_stream(resp).await
            }
            CodexWire::ChatCompletions => {
                let body = self.build_chat_body(messages, opts);
                let resp = self
                    .client
                    .post(format!("{}/chat/completions", self.base_url))
                    .bearer_auth(&self.access_token)
                    .json(&body)
                    .send()
                    .await?;
                let status = resp.status();
                let text = resp.text().await?;
                if !status.is_success() {
                    return Err(map_http_error(status, &text));
                }
                let parsed: ChatResponse = serde_json::from_str(&text)
                    .map_err(|e| anyhow::anyhow!("decode Codex chat: {e}; body={text}"))?;
                Ok(parsed.first_content())
            }
        }
    }
}

// ---- Responses API wire types ----

#[derive(Debug, Serialize)]
struct ResponsesRequest {
    model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
    input: Vec<InputItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    stream: bool,
    /// The ChatGPT codex backend rejects server-side storage; must be false.
    store: bool,
}

#[derive(Debug, Serialize)]
struct InputItem {
    role: String,
    content: Vec<ContentPart>,
}

impl InputItem {
    fn new(role: &str, text: &str) -> Self {
        Self {
            role: role.to_string(),
            content: vec![ContentPart {
                kind: "input_text".to_string(),
                text: text.to_string(),
            }],
        }
    }
}

#[derive(Debug, Serialize)]
struct ContentPart {
    #[serde(rename = "type")]
    kind: String,
    text: String,
}

// (Non-streaming Responses parsing removed: the codex backend only streams, so the
// SSE reader `read_responses_stream` parses events directly.)

// ---- Chat Completions wire types ----

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMsg>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Debug, Serialize)]
struct ChatMsg {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    #[serde(default)]
    choices: Vec<ChatChoice>,
}

impl ChatResponse {
    fn first_content(self) -> String {
        self.choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .unwrap_or_default()
    }
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatChoiceMsg,
}

#[derive(Debug, Deserialize)]
struct ChatChoiceMsg {
    #[serde(default)]
    content: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ChatProvider;

    fn write_fixture(dir: &std::path::Path) -> PathBuf {
        let p = dir.join("auth.json");
        // Fake, structurally-valid token (not a real secret); concat! keeps it out
        // of secret scanners.
        let tok = concat!("test-", "access-", "token");
        let acct = "11111111-2222-3333-4444-555555555555";
        let body = format!(
            r#"{{"auth_mode":"chatgpt","OPENAI_API_KEY":null,"tokens":{{"id_token":"x","access_token":"{tok}","refresh_token":"r","account_id":"{acct}"}},"last_refresh":"2026-06-01T00:00:00Z"}}"#
        );
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn from_auth_file_parses_and_is_cloud() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_fixture(dir.path());
        let prov = CodexOAuthProvider::from_auth_file(&p).unwrap();
        assert_eq!(prov.id(), "codex-oauth");
        assert!(!prov.is_local(), "codex-oauth must be cloud (SI-7)");
        assert_eq!(prov.wire(), CodexWire::Responses);
        assert_eq!(prov.model, DEFAULT_CODEX_MODEL);
        assert_eq!(prov.account_id, "11111111-2222-3333-4444-555555555555");
    }

    #[test]
    fn responses_body_folds_system_into_instructions() {
        let dir = tempfile::tempdir().unwrap();
        let prov = CodexOAuthProvider::from_auth_file(&write_fixture(dir.path())).unwrap();
        let msgs = [ChatMessage::system("be terse"), ChatMessage::user("hello")];
        let opts = ChatOpts::default().with_system("also be kind");
        let body = prov.build_responses_body(&msgs, &opts);
        let v = serde_json::to_value(&body).unwrap();
        let instr = v["instructions"].as_str().unwrap();
        assert!(instr.contains("be terse"));
        assert!(instr.contains("also be kind"));
        assert_eq!(v["input"][0]["role"], "user");
        assert_eq!(v["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(v["input"][0]["content"][0]["text"], "hello");
        // codex backend protocol requirements (verified live): stream + no store.
        assert_eq!(v["stream"], true);
        assert_eq!(v["store"], false);
        // max_output_tokens/temperature are rejected by the backend → omitted.
        assert!(v.get("max_output_tokens").is_none());
    }

    #[test]
    fn empty_system_still_sends_instructions() {
        // The backend 400s on missing instructions, so a user-only call must still
        // carry a default instructions string.
        let dir = tempfile::tempdir().unwrap();
        let prov = CodexOAuthProvider::from_auth_file(&write_fixture(dir.path())).unwrap();
        let body = prov.build_responses_body(&[ChatMessage::user("hi")], &ChatOpts::default());
        let v = serde_json::to_value(&body).unwrap();
        assert!(v["instructions"]
            .as_str()
            .map(|s| !s.is_empty())
            .unwrap_or(false));
    }

    #[test]
    fn with_base_url_picks_wire() {
        let dir = tempfile::tempdir().unwrap();
        let prov = CodexOAuthProvider::from_auth_file(&write_fixture(dir.path())).unwrap();
        let direct = prov.with_base_url("https://chatgpt.com/backend-api/codex");
        assert_eq!(direct.wire(), CodexWire::Responses);

        let dir2 = tempfile::tempdir().unwrap();
        let prov2 = CodexOAuthProvider::from_auth_file(&write_fixture(dir2.path())).unwrap();
        let wrapped = prov2.with_base_url("https://my-worker.workers.dev/v1");
        assert_eq!(wrapped.wire(), CodexWire::ChatCompletions);
    }

    #[test]
    fn expired_token_error_is_actionable() {
        let e = map_http_error(reqwest::StatusCode::UNAUTHORIZED, "nope");
        assert!(e.to_string().contains("codex login"));
        let e2 = map_http_error(reqwest::StatusCode::FORBIDDEN, "nope");
        assert!(e2.to_string().contains("codex login"));
        let e3 = map_http_error(reqwest::StatusCode::INTERNAL_SERVER_ERROR, "boom");
        assert!(e3.to_string().contains("500"));
    }

    #[test]
    fn missing_auth_file_errors_clearly() {
        let r = CodexOAuthProvider::from_auth_file(std::path::Path::new("/nonexistent/auth.json"));
        assert!(r.is_err());
        // Avoid unwrap_err (would require Debug on the provider; we deliberately
        // don't derive Debug so the access token can't leak into logs).
        assert!(r.err().unwrap().to_string().contains("codex login"));
    }

    // LIVE: calls the real ChatGPT backend via ~/.codex/auth.json. Sends only a
    // generic test prompt (no personal data). #[ignore] so it never runs in CI;
    // run manually to verify codex_oauth end-to-end.
    #[tokio::test]
    #[ignore = "live: calls ChatGPT backend via ~/.codex/auth.json; run manually"]
    async fn live_codex_completes() {
        let p = CodexOAuthProvider::from_default_auth().expect("read ~/.codex/auth.json");
        let r = p
            .complete(
                &[ChatMessage::user("Reply with exactly the word: PONG")],
                &ChatOpts::default().with_max_tokens(16),
            )
            .await;
        match r {
            Ok(s) => println!("LIVE_CODEX_OK len={} body={:?}", s.len(), s),
            Err(e) => println!("LIVE_CODEX_ERR {e}"),
        }
        // The point is the printed outcome; we don't assert on live content.
    }
}

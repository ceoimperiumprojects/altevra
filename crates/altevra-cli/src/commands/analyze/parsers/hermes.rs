//! Hermes session parser.
//!
//! Storage: `~/.hermes/sessions/session_<id>.json` — one JSON file per
//! session. Schema uses OpenAI tool_calls envelope:
//! ```json
//! {
//!   "id": "session_id",
//!   "created_at": "2026-05-27T10:00:00Z",
//!   "project_id": "revesta",
//!   "messages": [
//!     {"role":"user","content":"..."},
//!     {"role":"assistant","content":"...","tool_calls":[{"id":"tc_1","function":{"name":"x","arguments":"{}"}}]},
//!     {"role":"tool","tool_call_id":"tc_1","content":"..."}
//!   ]
//! }
//! ```

use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::path::Path;

use crate::commands::analyze::{ImportedSession, ImportedTurn};

#[derive(Debug, Deserialize)]
struct HermesSession {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    ended_at: Option<String>,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    project_name: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    messages: Vec<HermesMessage>,
}

#[derive(Debug, Deserialize)]
struct HermesMessage {
    role: String,
    #[serde(default)]
    content: serde_json::Value,
    #[serde(default)]
    tool_calls: Option<serde_json::Value>,
    #[serde(default)]
    tool_call_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
}

pub fn parse_file(path: &Path) -> anyhow::Result<Option<ImportedSession>> {
    let raw = std::fs::read_to_string(path)?;
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let sess: HermesSession = serde_json::from_str(&raw)
        .map_err(|e| anyhow::anyhow!("decode hermes session {}: {e}", path.display()))?;

    let external_id = sess
        .id
        .or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.strip_prefix("session_").unwrap_or(s).to_string())
        })
        .ok_or_else(|| anyhow::anyhow!("no hermes session id in {}", path.display()))?;

    let started_at = sess
        .created_at
        .as_deref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);

    let ended_at = sess
        .ended_at
        .as_deref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.with_timezone(&Utc));

    let turns: Vec<ImportedTurn> = sess
        .messages
        .into_iter()
        .enumerate()
        .map(|(idx, m)| {
            let role = match m.role.as_str() {
                "user" | "assistant" | "system" => m.role.clone(),
                "tool" => "tool_result".into(),
                other => other.to_string(),
            };
            let content = match &m.content {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Null => String::new(),
                v => v.to_string(),
            };
            let tool_name = m
                .name
                .clone()
                .or_else(|| m.tool_call_id.clone())
                .or_else(|| {
                    m.tool_calls
                        .as_ref()
                        .and_then(|tc| tc.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|f| f.get("function"))
                        .and_then(|f| f.get("name"))
                        .and_then(|n| n.as_str())
                        .map(String::from)
                });
            let ts = m
                .timestamp
                .as_deref()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|t| t.with_timezone(&Utc))
                .unwrap_or(started_at);
            ImportedTurn {
                turn_idx: idx as i64,
                role,
                content,
                tool_calls: m.tool_calls,
                tool_name,
                model: None,
                tokens_in: None,
                tokens_out: None,
                latency_ms: None,
                created_at: ts,
            }
        })
        .collect();

    if turns.is_empty() {
        return Ok(None);
    }

    Ok(Some(ImportedSession {
        external_id,
        tool_id: "hermes".into(),
        project_name: sess.project_name.or(sess.project_id),
        started_at,
        ended_at,
        model: sess.model,
        turns,
        imported_from: path.to_path_buf(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_json_temp(payload: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::Builder::new()
            .prefix("session_abc-")
            .suffix(".json")
            .tempfile()
            .unwrap();
        write!(f, "{payload}").unwrap();
        f
    }

    #[test]
    fn parse_simple_conversation() {
        let payload = r#"{
            "id":"hermes-1","created_at":"2026-05-27T10:00:00Z","project_id":"revesta",
            "messages":[
                {"role":"user","content":"hello"},
                {"role":"assistant","content":"hi","tool_calls":[{"id":"tc1","function":{"name":"Search","arguments":"{}"}}]},
                {"role":"tool","tool_call_id":"tc1","content":"results..."}
            ]
        }"#;
        let f = write_json_temp(payload);
        let parsed = parse_file(f.path()).unwrap().unwrap();
        assert_eq!(parsed.external_id, "hermes-1");
        assert_eq!(parsed.tool_id, "hermes");
        assert_eq!(parsed.turns.len(), 3);
        assert_eq!(parsed.turns[2].role, "tool_result");
        assert_eq!(parsed.turns[2].tool_name.as_deref(), Some("tc1"));
        assert_eq!(parsed.project_name.as_deref(), Some("revesta"));
    }

    #[test]
    fn parse_empty_messages_returns_none() {
        let payload = r#"{"id":"empty","messages":[]}"#;
        let f = write_json_temp(payload);
        assert!(parse_file(f.path()).unwrap().is_none());
    }

    #[test]
    fn parse_uses_filename_when_no_id() {
        let payload = r#"{"messages":[{"role":"user","content":"x"}]}"#;
        let f = write_json_temp(payload);
        let parsed = parse_file(f.path()).unwrap().unwrap();
        // Filename was `session_abc-XXXXX.json` so external_id should strip
        // the `session_` prefix.
        assert!(!parsed.external_id.starts_with("session_"));
    }
}

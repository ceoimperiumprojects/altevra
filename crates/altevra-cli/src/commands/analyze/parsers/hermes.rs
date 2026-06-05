//! Hermes session parser.
//!
//! Two storage formats live side-by-side under `~/.hermes/sessions/`:
//!
//! 1. **Single-JSON** (`session_<id>.json`) — OpenAI tool_calls envelope:
//!    ```json
//!    {
//!      "id": "session_id",
//!      "created_at": "2026-05-27T10:00:00Z",
//!      "project_id": "revesta",
//!      "messages": [
//!        {"role":"user","content":"..."},
//!        {"role":"assistant","content":"...","tool_calls":[{"id":"tc_1","function":{"name":"x","arguments":"{}"}}]},
//!        {"role":"tool","tool_call_id":"tc_1","content":"..."}
//!      ]
//!    }
//!    ```
//!    Parsed by [`parse_file`].
//!
//! 2. **JSONL** (`YYYYMMDD_HHMMSS_<hex>.jsonl`) — one record per line. The
//!    first line is a `session_meta` envelope carrying `model`/`platform`/
//!    `tools`/`timestamp`. Subsequent lines are messages with
//!    `role` ∈ {user, assistant, tool, system}, `content`, `timestamp`, and
//!    optionally `tool_calls` / `tool_call_id` / `name`. Parsed by
//!    [`parse_session_jsonl`].

use chrono::{DateTime, NaiveDateTime, Utc};
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

/// Parse a Hermes JSONL session file. The filename pattern is
/// `YYYYMMDD_HHMMSS_<hex>.jsonl`; we use that timestamp as the session start
/// when the first message lacks a timestamp. Returns `Ok(None)` when the
/// file is empty or contains zero usable message lines.
pub fn parse_session_jsonl(path: &Path) -> anyhow::Result<Option<ImportedSession>> {
    let raw = std::fs::read_to_string(path)?;
    if raw.trim().is_empty() {
        return Ok(None);
    }

    // External id from filename stem (`YYYYMMDD_HHMMSS_<hex>`). Required for
    // idempotent upsert + filename-derived start-time fallback.
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow::anyhow!("hermes jsonl missing filename: {}", path.display()))?
        .to_string();
    let filename_started_at = parse_filename_timestamp(&stem);

    let mut meta_model: Option<String> = None;
    let mut meta_started_at: Option<DateTime<Utc>> = None;
    let mut turns: Vec<ImportedTurn> = Vec::new();

    for (line_idx, line) in raw.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                // Tolerate corrupt lines — Hermes occasionally truncates on crash.
                continue;
            }
        };
        let role = value
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let ts_str = value.get("timestamp").and_then(|v| v.as_str());
        let ts = ts_str
            .and_then(parse_loose_timestamp)
            .or(meta_started_at)
            .or(filename_started_at)
            .unwrap_or_else(Utc::now);

        if role == "session_meta" {
            if let Some(m) = value.get("model").and_then(|v| v.as_str()) {
                meta_model = Some(m.to_string());
            }
            if meta_started_at.is_none() {
                meta_started_at = ts_str.and_then(parse_loose_timestamp);
            }
            continue;
        }
        if role.is_empty() {
            continue;
        }

        let normalized_role = match role.as_str() {
            "user" | "assistant" | "system" => role.clone(),
            "tool" => "tool_result".into(),
            other => other.to_string(),
        };

        let content = match value.get("content") {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(serde_json::Value::Null) | None => String::new(),
            Some(v) => v.to_string(),
        };

        let tool_calls = value.get("tool_calls").cloned().and_then(|v| {
            if v.is_null() {
                None
            } else {
                Some(v)
            }
        });
        let tool_name = value
            .get("name")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| {
                value
                    .get("tool_call_id")
                    .and_then(|v| v.as_str())
                    .map(String::from)
            })
            .or_else(|| {
                tool_calls
                    .as_ref()
                    .and_then(|tc| tc.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|f| f.get("function"))
                    .and_then(|f| f.get("name"))
                    .and_then(|n| n.as_str())
                    .map(String::from)
            });

        turns.push(ImportedTurn {
            turn_idx: line_idx as i64,
            role: normalized_role,
            content,
            tool_calls,
            tool_name,
            model: None,
            tokens_in: None,
            tokens_out: None,
            latency_ms: None,
            created_at: ts,
        });
    }

    if turns.is_empty() {
        return Ok(None);
    }

    // Re-index turns 0..N after meta/empty lines are filtered out.
    for (i, t) in turns.iter_mut().enumerate() {
        t.turn_idx = i as i64;
    }

    let started_at = meta_started_at
        .or(filename_started_at)
        .unwrap_or_else(|| turns[0].created_at);
    let ended_at = turns.last().map(|t| t.created_at);

    Ok(Some(ImportedSession {
        external_id: stem,
        tool_id: "hermes".into(),
        project_name: None,
        started_at,
        ended_at,
        model: meta_model,
        turns,
        imported_from: path.to_path_buf(),
    }))
}

/// `YYYYMMDD_HHMMSS_<hex>` → UTC. Used for filtering by `--since` without
/// reading the file body.
pub fn parse_filename_timestamp(stem: &str) -> Option<DateTime<Utc>> {
    // Strip optional `session_` prefix that some legacy files carry.
    let s = stem.strip_prefix("session_").unwrap_or(stem);
    let head: String = s.chars().take(15).collect();
    if head.len() < 15 || head.chars().nth(8) != Some('_') {
        return None;
    }
    NaiveDateTime::parse_from_str(&head, "%Y%m%d_%H%M%S")
        .ok()
        .map(|n| DateTime::<Utc>::from_naive_utc_and_offset(n, Utc))
}

/// Accepts RFC3339 (`...Z` or `...+00:00`) as well as the naive `%Y-%m-%dT%H:%M:%S%.f`
/// that Hermes JSONL emits without a timezone — treated as UTC.
fn parse_loose_timestamp(s: &str) -> Option<DateTime<Utc>> {
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Some(t.with_timezone(&Utc));
    }
    if let Ok(n) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f") {
        return Some(DateTime::<Utc>::from_naive_utc_and_offset(n, Utc));
    }
    if let Ok(n) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return Some(DateTime::<Utc>::from_naive_utc_and_offset(n, Utc));
    }
    None
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

    #[test]
    fn filename_timestamp_parses() {
        let t = parse_filename_timestamp("20260518_123317_bbf2c115").unwrap();
        assert_eq!(t.to_rfc3339(), "2026-05-18T12:33:17+00:00");
    }

    #[test]
    fn filename_timestamp_strips_session_prefix() {
        assert!(parse_filename_timestamp("session_20260518_123317_abc").is_some());
    }

    #[test]
    fn filename_timestamp_rejects_short() {
        assert!(parse_filename_timestamp("notatimestamp").is_none());
    }

    #[test]
    fn loose_timestamp_accepts_naive_microseconds() {
        let t = parse_loose_timestamp("2026-05-18T12:33:38.997572").unwrap();
        assert_eq!(t.timezone(), Utc);
    }

    fn write_jsonl_temp(name: &str, lines: &[&str]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
        (dir, path)
    }

    #[test]
    fn jsonl_parse_meta_then_messages() {
        let lines = [
            r#"{"role":"session_meta","model":"gpt-5.5","tools":[],"timestamp":"2026-05-18T12:33:38.997572"}"#,
            r#"{"role":"user","content":"hi","timestamp":"2026-05-18T12:33:39.000000"}"#,
            r#"{"role":"assistant","content":"hello","tool_calls":[{"function":{"name":"search","arguments":"{}"}}],"timestamp":"2026-05-18T12:33:40.000000"}"#,
            r#"{"role":"tool","content":"ok","tool_call_id":"call_X","name":"search","timestamp":"2026-05-18T12:33:41.000000"}"#,
        ];
        let (_dir, p) = write_jsonl_temp("20260518_123317_abc.jsonl", &lines);
        let sess = parse_session_jsonl(&p).unwrap().unwrap();
        assert_eq!(sess.external_id, "20260518_123317_abc");
        assert_eq!(sess.tool_id, "hermes");
        assert_eq!(sess.model.as_deref(), Some("gpt-5.5"));
        assert_eq!(sess.turns.len(), 3);
        assert_eq!(sess.turns[0].role, "user");
        assert_eq!(sess.turns[2].role, "tool_result");
        assert_eq!(sess.turns[2].tool_name.as_deref(), Some("search"));
        // Indices are dense 0..N (session_meta filtered out).
        assert_eq!(sess.turns[0].turn_idx, 0);
        assert_eq!(sess.turns[1].turn_idx, 1);
        assert_eq!(sess.turns[2].turn_idx, 2);
    }

    #[test]
    fn jsonl_parse_empty_returns_none() {
        let (_dir, p) = write_jsonl_temp("20260518_123317_xyz.jsonl", &[]);
        assert!(parse_session_jsonl(&p).unwrap().is_none());
    }

    #[test]
    fn jsonl_parse_tolerates_corrupt_line() {
        let lines = [
            r#"{"role":"session_meta","model":"gpt-5.5","timestamp":"2026-05-18T12:33:38"}"#,
            r#"{ corrupt"#,
            r#"{"role":"user","content":"good","timestamp":"2026-05-18T12:33:39"}"#,
        ];
        let (_dir, p) = write_jsonl_temp("20260518_123317_def.jsonl", &lines);
        let sess = parse_session_jsonl(&p).unwrap().unwrap();
        assert_eq!(sess.turns.len(), 1);
        assert_eq!(sess.turns[0].content, "good");
    }
}

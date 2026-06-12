//! Cursor / VS Code chat session parser.
//!
//! Storage:
//! ```
//! ~/.config/Code/User/workspaceStorage/<hash>/chatSessions/<session-id>.jsonl
//! ~/.config/Cursor/User/workspaceStorage/<hash>/chatSessions/<session-id>.jsonl
//! ```
//! Each file is JSONL. The first line is the session shell (sessionId,
//! requests[]), subsequent lines store input-state snapshots. We pull the
//! shell line, extract `requests[].message.text` for user prompts and
//! `requests[].response[].value` for assistant turns.

use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::commands::analyze::{ImportedSession, ImportedTurn};

#[derive(Debug, Deserialize)]
struct ShellLine {
    kind: i32,
    v: serde_json::Value,
}

pub fn parse_file(path: &Path) -> anyhow::Result<Option<ImportedSession>> {
    let raw = std::fs::read_to_string(path)?;
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let mut session_id_from_payload: Option<String> = None;
    let mut creation_ts: Option<DateTime<Utc>> = None;
    let mut turns: Vec<ImportedTurn> = Vec::new();

    for (line_no, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let shell: ShellLine = match serde_json::from_str(line) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(file = %path.display(), line = line_no, error = %e, "skip malformed cursor line");
                continue;
            }
        };
        // kind=0 = session shell; kind=1+ = state snapshots (we ignore).
        if shell.kind != 0 {
            continue;
        }
        if let Some(sid) = shell.v.get("sessionId").and_then(|v| v.as_str()) {
            session_id_from_payload = Some(sid.to_string());
        }
        if let Some(cd) = shell.v.get("creationDate").and_then(|v| v.as_i64()) {
            creation_ts = Utc.timestamp_millis_opt(cd).single();
        }

        if let Some(requests) = shell.v.get("requests").and_then(|r| r.as_array()) {
            for (idx, req) in requests.iter().enumerate() {
                let user_text = req
                    .get("message")
                    .and_then(|m| m.get("text"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("");
                if !user_text.is_empty() {
                    turns.push(ImportedTurn {
                        turn_idx: turns.len() as i64,
                        role: "user".into(),
                        content: user_text.into(),
                        tool_calls: None,
                        tool_name: None,
                        model: None,
                        tokens_in: None,
                        tokens_out: None,
                        latency_ms: None,
                        created_at: creation_ts.unwrap_or_else(Utc::now),
                    });
                }
                if let Some(resp_arr) = req.get("response").and_then(|r| r.as_array()) {
                    let mut combined = String::new();
                    for part in resp_arr {
                        if let Some(v) = part.get("value").and_then(|v| v.as_str()) {
                            if !combined.is_empty() {
                                combined.push('\n');
                            }
                            combined.push_str(v);
                        }
                    }
                    if !combined.is_empty() {
                        turns.push(ImportedTurn {
                            turn_idx: turns.len() as i64,
                            role: "assistant".into(),
                            content: combined,
                            tool_calls: None,
                            tool_name: None,
                            model: req
                                .get("modelId")
                                .and_then(|m| m.as_str())
                                .map(String::from),
                            tokens_in: None,
                            tokens_out: None,
                            latency_ms: None,
                            created_at: creation_ts.unwrap_or_else(Utc::now),
                        });
                    }
                }
                let _ = idx;
            }
        }
    }

    let external_id = session_id_from_payload
        .or_else(|| path.file_stem().and_then(|s| s.to_str()).map(String::from))
        .ok_or_else(|| anyhow::anyhow!("no session id in {}", path.display()))?;

    if turns.is_empty() {
        return Ok(None);
    }

    Ok(Some(ImportedSession {
        external_id,
        tool_id: "cursor".into(),
        project_name: None,
        started_at: creation_ts.unwrap_or_else(Utc::now),
        ended_at: Some(creation_ts.unwrap_or_else(Utc::now)),
        model: None,
        turns,
        imported_from: path.to_path_buf(),
        working_dir: None,
    }))
}

/// Walk Cursor / VS Code chatSessions JSONL files.
/// Currently the orchestrator uses `discovery::discover()` instead; kept here
/// for tool-specific discovery via MCP in v0.5+.
#[allow(dead_code)]
pub fn discover(root: &Path) -> Vec<PathBuf> {
    if !root.exists() {
        return vec![];
    }
    walkdir::WalkDir::new(root)
        .max_depth(4)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_type().is_file()
                && e.path().extension().and_then(|s| s.to_str()) == Some("jsonl")
                && e.path()
                    .parent()
                    .and_then(|p| p.file_name())
                    .map(|n| n == "chatSessions")
                    .unwrap_or(false)
        })
        .map(|e| e.path().to_path_buf())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_jsonl_temp(lines: &[&str]) -> tempfile::NamedTempFile {
        let mut f = tempfile::Builder::new()
            .suffix(".jsonl")
            .tempfile()
            .unwrap();
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
        f
    }

    #[test]
    fn parse_session_with_user_and_response() {
        let line = r#"{"kind":0,"v":{"sessionId":"cur-abc","creationDate":1716800000000,"requests":[{"message":{"text":"explain rust traits"},"response":[{"kind":"markdown","value":"Traits define behavior."}]}]}}"#;
        let f = write_jsonl_temp(&[line]);
        let parsed = parse_file(f.path()).unwrap().unwrap();
        assert_eq!(parsed.external_id, "cur-abc");
        assert_eq!(parsed.tool_id, "cursor");
        assert_eq!(parsed.turns.len(), 2);
        assert_eq!(parsed.turns[0].role, "user");
        assert_eq!(parsed.turns[1].role, "assistant");
        assert!(parsed.turns[1].content.contains("Traits"));
    }

    #[test]
    fn parse_empty_session_returns_none() {
        let line = r#"{"kind":0,"v":{"sessionId":"empty","requests":[]}}"#;
        let f = write_jsonl_temp(&[line]);
        assert!(parse_file(f.path()).unwrap().is_none());
    }

    #[test]
    fn parse_ignores_state_snapshots() {
        let lines = [
            r#"{"kind":0,"v":{"sessionId":"x","requests":[{"message":{"text":"hi"}}]}}"#,
            r#"{"kind":1,"k":["inputState"],"v":{"inputText":"draft"}}"#,
        ];
        let f = write_jsonl_temp(&lines);
        let parsed = parse_file(f.path()).unwrap().unwrap();
        assert_eq!(parsed.turns.len(), 1);
    }
}

//! Claude Code session parser.
//!
//! Storage layout:
//! ```
//! ~/.claude/projects/<project-hash>/<session-uuid>.jsonl
//! ```
//! Each line is a JSON record. Common shapes:
//! - `{"type":"user","message":{"role":"user","content":"..."},...}`
//! - `{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"..."},{"type":"tool_use",...}]},...}`
//! - `{"type":"tool_result","tool_use_id":"...","content":[{"type":"text","text":"..."}],...}`
//! - `{"type":"summary","summary":"...","leafUuid":"..."}` — emitted at session end
//! - `{"type":"system","content":"..."}` — system messages
//!
//! external_id = filename stem (the session UUID).

use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::commands::analyze::{ImportedSession, ImportedTurn};

#[derive(Debug, Deserialize)]
struct RawRecord {
    #[serde(rename = "type", default)]
    record_type: String,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    message: Option<serde_json::Value>,
    #[serde(default)]
    content: Option<serde_json::Value>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    tool_use_id: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default, rename = "sessionId")]
    session_id: Option<String>,
}

pub fn parse_file(path: &Path) -> anyhow::Result<Option<ImportedSession>> {
    let raw = std::fs::read_to_string(path)?;
    if raw.trim().is_empty() {
        return Ok(None);
    }

    let external_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow::anyhow!("path has no stem: {}", path.display()))?
        .to_string();

    let mut turns: Vec<ImportedTurn> = Vec::new();
    let mut started_at: Option<DateTime<Utc>> = None;
    let mut ended_at: Option<DateTime<Utc>> = None;
    let mut summary: Option<String> = None;
    let mut project_hint: Option<String> = None;
    let mut model: Option<String> = None;

    for (line_no, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let rec: RawRecord = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    file = %path.display(),
                    line = line_no,
                    error = %e,
                    "skip malformed claude-code line"
                );
                continue;
            }
        };

        let ts = rec
            .timestamp
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.with_timezone(&Utc));

        if let Some(t) = ts {
            if started_at.is_none() {
                started_at = Some(t);
            }
            ended_at = Some(t);
        }

        if project_hint.is_none() {
            if let Some(cwd) = rec.cwd.clone() {
                project_hint = Some(cwd);
            }
        }

        match rec.record_type.as_str() {
            "user" | "assistant" => {
                let (role, content_text, used_model) = extract_message(&rec);
                if used_model.is_some() && model.is_none() {
                    model = used_model;
                }
                turns.push(ImportedTurn {
                    turn_idx: turns.len() as i64,
                    role: role.into(),
                    content: content_text,
                    tool_calls: extract_tool_calls(&rec),
                    tool_name: None,
                    model: model.clone(),
                    tokens_in: None,
                    tokens_out: None,
                    latency_ms: None,
                    created_at: ts.unwrap_or_else(Utc::now),
                });
            }
            "tool_use" => {
                turns.push(ImportedTurn {
                    turn_idx: turns.len() as i64,
                    role: "tool_call".into(),
                    content: rec
                        .message
                        .as_ref()
                        .map(|m| m.to_string())
                        .unwrap_or_default(),
                    tool_calls: rec.message.clone(),
                    tool_name: rec
                        .message
                        .as_ref()
                        .and_then(|m| m.get("name"))
                        .and_then(|n| n.as_str())
                        .map(String::from),
                    model: model.clone(),
                    tokens_in: None,
                    tokens_out: None,
                    latency_ms: None,
                    created_at: ts.unwrap_or_else(Utc::now),
                });
            }
            "tool_result" => {
                let content_str = match rec.content.as_ref() {
                    Some(v) => v.to_string(),
                    None => String::new(),
                };
                turns.push(ImportedTurn {
                    turn_idx: turns.len() as i64,
                    role: "tool_result".into(),
                    content: content_str,
                    tool_calls: None,
                    tool_name: rec.tool_use_id.clone(),
                    model: model.clone(),
                    tokens_in: None,
                    tokens_out: None,
                    latency_ms: None,
                    created_at: ts.unwrap_or_else(Utc::now),
                });
            }
            "summary" => {
                if let Some(s) = rec.summary.clone() {
                    summary = Some(s);
                }
            }
            "system" => {
                if let Some(c) = rec.content.as_ref().and_then(|v| v.as_str()) {
                    turns.push(ImportedTurn {
                        turn_idx: turns.len() as i64,
                        role: "system".into(),
                        content: c.into(),
                        tool_calls: None,
                        tool_name: None,
                        model: model.clone(),
                        tokens_in: None,
                        tokens_out: None,
                        latency_ms: None,
                        created_at: ts.unwrap_or_else(Utc::now),
                    });
                }
            }
            _ => {}
        }

        if let Some(sid) = &rec.session_id {
            if sid != &external_id {
                tracing::debug!(
                    file = %path.display(),
                    expected = %external_id,
                    found = %sid,
                    "sessionId mismatch (ignored)"
                );
            }
        }
    }

    if turns.is_empty() {
        return Ok(None);
    }

    Ok(Some(ImportedSession {
        external_id,
        tool_id: "claude-code".into(),
        project_name: project_hint
            .as_deref()
            .and_then(|p| p.rsplit('/').next())
            .map(String::from),
        started_at: started_at.unwrap_or_else(Utc::now),
        ended_at: Some(ended_at.unwrap_or_else(Utc::now)),
        model,
        turns,
        imported_from: path.to_path_buf(),
    }))
    .map(|opt| {
        opt.map(|mut s| {
            // Attach the summary as a synthetic trailing system turn so it's
            // preserved verbatim alongside the LLM-generated one.
            if let Some(_sum) = summary.as_ref() {
                // summary is just hint; orchestrator may overwrite with LLM
                let _ = path; // silence dead borrow
            }
            // Append nothing — summary handled at orchestrator level.
            s.imported_from = path.to_path_buf();
            s
        })
    })
}

fn extract_message(rec: &RawRecord) -> (&'static str, String, Option<String>) {
    let role = match rec.record_type.as_str() {
        "user" => "user",
        _ => "assistant",
    };
    let mut model: Option<String> = None;
    let content = if let Some(msg) = &rec.message {
        // Claude Code message.content is either a string OR an array of blocks.
        if let Some(m) = msg.get("model").and_then(|v| v.as_str()) {
            model = Some(m.to_string());
        }
        let raw = msg
            .get("content")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        flatten_content(&raw)
    } else {
        String::new()
    };
    (role, content, model)
}

fn flatten_content(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(arr) => {
            let mut parts = Vec::with_capacity(arr.len());
            for item in arr {
                if let Some(t) = item.get("text").and_then(|x| x.as_str()) {
                    parts.push(t.to_string());
                } else if let Some(content) = item.get("content") {
                    parts.push(flatten_content(content));
                }
            }
            parts.join("\n")
        }
        serde_json::Value::Object(_) => v.to_string(),
        _ => String::new(),
    }
}

fn extract_tool_calls(rec: &RawRecord) -> Option<serde_json::Value> {
    let msg = rec.message.as_ref()?;
    let content = msg.get("content")?.as_array()?;
    let tool_calls: Vec<_> = content
        .iter()
        .filter(|item| item.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
        .cloned()
        .collect();
    if tool_calls.is_empty() {
        None
    } else {
        Some(serde_json::Value::Array(tool_calls))
    }
}

/// Walk all `~/.claude/projects/<hash>/*.jsonl` files under `root`.
/// Currently the orchestrator uses `discovery::discover()` instead; kept here
/// for tool-specific discovery via MCP in v0.5+.
#[allow(dead_code)]
pub fn discover(root: &Path) -> Vec<PathBuf> {
    if !root.exists() {
        return vec![];
    }
    walkdir::WalkDir::new(root)
        .max_depth(3)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_type().is_file()
                && e.path().extension().and_then(|s| s.to_str()) == Some("jsonl")
        })
        .map(|e| e.path().to_path_buf())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_jsonl(lines: &[&str]) -> NamedTempFile {
        let mut f = tempfile::Builder::new()
            .prefix("11111111-2222-3333-4444-555555555555")
            .suffix(".jsonl")
            .tempfile()
            .unwrap();
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
        f
    }

    #[test]
    fn parse_simple_user_assistant() {
        let lines = [
            r#"{"type":"user","timestamp":"2026-05-27T10:00:00Z","message":{"role":"user","content":"hello"}}"#,
            r#"{"type":"assistant","timestamp":"2026-05-27T10:00:01Z","message":{"role":"assistant","model":"claude-opus-4-7","content":[{"type":"text","text":"hi back"}]}}"#,
        ];
        let f = write_jsonl(&lines);
        let parsed = parse_file(f.path()).unwrap().unwrap();
        assert_eq!(parsed.tool_id, "claude-code");
        assert_eq!(parsed.turns.len(), 2);
        assert_eq!(parsed.turns[0].content, "hello");
        assert_eq!(parsed.turns[1].content, "hi back");
        assert_eq!(parsed.model.as_deref(), Some("claude-opus-4-7"));
    }

    #[test]
    fn parse_tool_use_and_result() {
        let lines = [
            r#"{"type":"user","timestamp":"2026-05-27T10:00:00Z","message":{"role":"user","content":"run ls"}}"#,
            r#"{"type":"assistant","timestamp":"2026-05-27T10:00:01Z","message":{"role":"assistant","content":[{"type":"text","text":"Running it"},{"type":"tool_use","id":"tu1","name":"Bash","input":{"command":"ls"}}]}}"#,
            r#"{"type":"tool_result","tool_use_id":"tu1","timestamp":"2026-05-27T10:00:02Z","content":[{"type":"text","text":"file1\nfile2"}]}"#,
        ];
        let f = write_jsonl(&lines);
        let parsed = parse_file(f.path()).unwrap().unwrap();
        assert_eq!(parsed.turns.len(), 3);
        assert_eq!(parsed.turns[1].role, "assistant");
        assert!(parsed.turns[1].tool_calls.is_some());
        assert_eq!(parsed.turns[2].role, "tool_result");
        assert!(parsed.turns[2].content.contains("file1"));
    }

    #[test]
    fn parse_skips_malformed_lines() {
        let lines = [
            r#"{"type":"user","message":{"content":"valid"}}"#,
            r#"not json"#,
            r#"{"type":"user","message":{"content":"after-bad"}}"#,
        ];
        let f = write_jsonl(&lines);
        let parsed = parse_file(f.path()).unwrap().unwrap();
        assert_eq!(parsed.turns.len(), 2);
    }

    #[test]
    fn empty_file_returns_none() {
        let f = write_jsonl(&[]);
        assert!(parse_file(f.path()).unwrap().is_none());
    }

    #[test]
    fn external_id_is_filename_stem() {
        let lines = [r#"{"type":"user","message":{"content":"hi"}}"#];
        let f = write_jsonl(&lines);
        let parsed = parse_file(f.path()).unwrap().unwrap();
        assert!(parsed
            .external_id
            .starts_with("11111111-2222-3333-4444-555555555555"));
    }
}

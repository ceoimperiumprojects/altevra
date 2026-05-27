//! Antigravity (gemini-cli) history parser.
//!
//! Storage: `~/.gemini/antigravity-cli/history.jsonl` — one append-only log
//! across all conversations. Records are grouped by `conversationId`.
//! Antigravity stores user inputs only (no assistant responses persisted).

use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

use crate::commands::analyze::{ImportedSession, ImportedTurn};

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum FlexTs {
    Str(String),
    Int(i64),
}

#[derive(Debug, Deserialize)]
struct AGRecord {
    #[serde(default, rename = "conversationId")]
    conversation_id: Option<String>,
    #[serde(default)]
    timestamp: Option<FlexTs>,
    /// Antigravity uses `display` (user-visible text). Some older builds used `text`.
    #[serde(default)]
    display: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    role: Option<String>,
    /// Antigravity uses `workspace`. Older builds used `cwd`.
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
}

fn flex_ts_to_utc(t: &FlexTs) -> Option<DateTime<Utc>> {
    match t {
        FlexTs::Str(s) => DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| d.with_timezone(&Utc)),
        FlexTs::Int(ms) => Utc.timestamp_millis_opt(*ms).single(),
    }
}

pub fn parse_file(path: &Path) -> anyhow::Result<Vec<ImportedSession>> {
    let raw = std::fs::read_to_string(path)?;
    if raw.trim().is_empty() {
        return Ok(vec![]);
    }
    let mut grouped: BTreeMap<String, Vec<(AGRecord, DateTime<Utc>)>> = BTreeMap::new();
    let mut fallback_counter = 0usize;

    for (line_no, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let rec: AGRecord = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(file = %path.display(), line = line_no, error = %e, "skip malformed antigravity line");
                continue;
            }
        };
        let conv = rec.conversation_id.clone().unwrap_or_else(|| {
            fallback_counter += 1;
            format!("antigravity-anon-{fallback_counter}")
        });
        let ts = rec
            .timestamp
            .as_ref()
            .and_then(flex_ts_to_utc)
            .unwrap_or_else(Utc::now);
        grouped.entry(conv).or_default().push((rec, ts));
    }

    let mut sessions = Vec::new();
    for (conv_id, mut recs) in grouped {
        recs.sort_by_key(|(_, t)| *t);
        if recs.is_empty() {
            continue;
        }
        let started_at = recs[0].1;
        let ended_at = recs.last().unwrap().1;
        let cwd = recs
            .iter()
            .find_map(|(r, _)| r.workspace.clone().or_else(|| r.cwd.clone()));

        let turns: Vec<ImportedTurn> = recs
            .into_iter()
            .enumerate()
            .map(|(idx, (rec, ts))| {
                let content = rec.display.or(rec.text).unwrap_or_default();
                ImportedTurn {
                    turn_idx: idx as i64,
                    role: rec.role.unwrap_or_else(|| "user".into()),
                    content,
                    tool_calls: None,
                    tool_name: None,
                    model: None,
                    tokens_in: None,
                    tokens_out: None,
                    latency_ms: None,
                    created_at: ts,
                }
            })
            .collect();

        let workspace = cwd.clone();
        sessions.push(ImportedSession {
            external_id: conv_id,
            tool_id: "antigravity".into(),
            project_name: workspace.as_deref().and_then(|c| c.rsplit('/').next()).map(String::from),
            started_at,
            ended_at: Some(ended_at),
            model: None,
            turns,
            imported_from: path.to_path_buf(),
        });
    }
    Ok(sessions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp(lines: &[&str]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
        f
    }

    #[test]
    fn groups_by_conversation_id() {
        let lines = [
            r#"{"conversationId":"conv-1","timestamp":"2026-05-27T10:00:00Z","text":"first","role":"user"}"#,
            r#"{"conversationId":"conv-2","timestamp":"2026-05-27T10:01:00Z","text":"other","role":"user"}"#,
            r#"{"conversationId":"conv-1","timestamp":"2026-05-27T10:02:00Z","text":"second","role":"user"}"#,
        ];
        let f = write_temp(&lines);
        let sessions = parse_file(f.path()).unwrap();
        assert_eq!(sessions.len(), 2);
        let conv1 = sessions.iter().find(|s| s.external_id == "conv-1").unwrap();
        assert_eq!(conv1.turns.len(), 2);
        assert_eq!(conv1.turns[0].content, "first");
        assert_eq!(conv1.turns[1].content, "second");
    }

    #[test]
    fn handles_anonymous_records() {
        let lines = [r#"{"display":"orphan","role":"user"}"#];
        let f = write_temp(&lines);
        let sessions = parse_file(f.path()).unwrap();
        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].external_id.starts_with("antigravity-anon-"));
    }

    #[test]
    fn accepts_integer_timestamps_and_display_field() {
        // Real antigravity-cli format uses unix-millis ints + `display` field.
        let lines = [
            r#"{"display":"hello","timestamp":1779222267697,"workspace":"/home/x","conversationId":"conv-X"}"#,
            r#"{"display":"second","timestamp":1779222370000,"conversationId":"conv-X"}"#,
        ];
        let f = write_temp(&lines);
        let sessions = parse_file(f.path()).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].turns.len(), 2);
        assert_eq!(sessions[0].turns[0].content, "hello");
    }

    #[test]
    fn empty_file_returns_empty_vec() {
        let f = write_temp(&[]);
        let sessions = parse_file(f.path()).unwrap();
        assert!(sessions.is_empty());
    }
}

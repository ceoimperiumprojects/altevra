//! Codex CLI session parser.
//!
//! Storage:
//! - `~/.codex/state_5.sqlite` — `threads` table with thread metadata
//!   (id, title, project, cwd, created_at). NO `messages` / `turns` table
//!   with actual conversation content — Codex stores those exclusively in
//!   `history.jsonl`. So turn counts depend entirely on how complete that
//!   append-only file is. If `history.jsonl` was truncated, deleted, or
//!   never enabled, sessions will appear with 0-1 turns even though the
//!   thread metadata indicates real activity.
//! - `~/.codex/history.jsonl` — append-only log of all interactions,
//!   keyed by `thread_id`. Source of truth for conversation flow.
//! - `~/.codex/logs_2.sqlite` — tool call telemetry (not currently used;
//!   could be joined for tool-call enrichment in v0.5+).
//!
//! Strategy: read `history.jsonl`, group lines by `thread_id` field. The
//! state SQLite is advisory — enriches thread title/project/created_at,
//! never required.

use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

use crate::commands::analyze::{ImportedSession, ImportedTurn};

#[derive(Debug, Deserialize)]
struct HistoryLine {
    #[serde(default, alias = "thread_id", alias = "threadId")]
    thread_id: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    content: serde_json::Value,
    #[serde(default)]
    tool_name: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Debug, Default, Clone)]
struct ThreadMetadata {
    /// Reserved — Codex `threads.title` value, surfaced via MCP in v0.5+.
    #[allow(dead_code)]
    title: Option<String>,
    project: Option<String>,
    created_at: Option<DateTime<Utc>>,
}

fn load_thread_metadata(state_db: &Path) -> BTreeMap<String, ThreadMetadata> {
    let mut out = BTreeMap::new();
    let conn =
        match Connection::open_with_flags(state_db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(db = %state_db.display(), error = %e, "cannot open codex state db");
                return out;
            }
        };
    let mut stmt = match conn.prepare("SELECT id, title, project, created_at FROM threads") {
        Ok(s) => s,
        Err(_) => {
            // Schema may vary — try a forgiving query.
            return out;
        }
    };
    let rows = stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let title: Option<String> = row.get(1).ok();
        let project: Option<String> = row.get(2).ok();
        let created: Option<String> = row.get(3).ok();
        Ok((id, title, project, created))
    });
    if let Ok(iter) = rows {
        for r in iter.flatten() {
            let (id, title, project, created) = r;
            let created_at = created
                .as_deref()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|t| t.with_timezone(&Utc));
            out.insert(
                id,
                ThreadMetadata {
                    title,
                    project,
                    created_at,
                },
            );
        }
    }
    out
}

/// Parse history.jsonl. Optionally enriches with thread metadata from the
/// state SQLite db (state_db can be None).
pub fn parse_history(
    history_path: &Path,
    state_db: Option<&Path>,
) -> anyhow::Result<Vec<ImportedSession>> {
    let raw = std::fs::read_to_string(history_path)?;
    if raw.trim().is_empty() {
        return Ok(vec![]);
    }
    let metadata = state_db.map(load_thread_metadata).unwrap_or_default();

    let mut grouped: BTreeMap<String, Vec<(HistoryLine, DateTime<Utc>)>> = BTreeMap::new();
    let mut fallback = 0usize;

    for (line_no, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let rec: HistoryLine = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(file = %history_path.display(), line = line_no, error = %e, "skip malformed codex line");
                continue;
            }
        };
        let tid = rec.thread_id.clone().unwrap_or_else(|| {
            fallback += 1;
            format!("codex-anon-{fallback}")
        });
        let ts = rec
            .timestamp
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);
        grouped.entry(tid).or_default().push((rec, ts));
    }

    let mut sessions = Vec::new();
    for (thread_id, mut recs) in grouped {
        recs.sort_by_key(|(_, t)| *t);
        let started_at = metadata
            .get(&thread_id)
            .and_then(|m| m.created_at)
            .or_else(|| recs.first().map(|(_, t)| *t))
            .unwrap_or_else(Utc::now);
        let ended_at = recs.last().map(|(_, t)| *t);

        let model = recs.iter().find_map(|(r, _)| r.model.clone());

        let turns: Vec<ImportedTurn> = recs
            .into_iter()
            .enumerate()
            .map(|(idx, (rec, ts))| {
                let role = rec.role.unwrap_or_else(|| "user".into());
                let content_str = match &rec.content {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Null => String::new(),
                    v => v.to_string(),
                };
                ImportedTurn {
                    turn_idx: idx as i64,
                    role,
                    content: content_str,
                    tool_calls: None,
                    tool_name: rec.tool_name,
                    model: rec.model.or_else(|| model.clone()),
                    tokens_in: None,
                    tokens_out: None,
                    latency_ms: None,
                    created_at: ts,
                }
            })
            .collect();

        if turns.is_empty() {
            continue;
        }

        sessions.push(ImportedSession {
            external_id: thread_id.clone(),
            tool_id: "codex".into(),
            project_name: metadata.get(&thread_id).and_then(|m| m.project.clone()),
            started_at,
            ended_at,
            model,
            turns,
            imported_from: history_path.to_path_buf(),
        });
    }

    Ok(sessions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_history(lines: &[&str]) -> tempfile::NamedTempFile {
        let mut f = tempfile::Builder::new()
            .prefix("codex-history-")
            .suffix(".jsonl")
            .tempfile()
            .unwrap();
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
        f
    }

    #[test]
    fn groups_by_thread_id() {
        let lines = [
            r#"{"thread_id":"t-1","timestamp":"2026-05-27T10:00:00Z","role":"user","content":"first"}"#,
            r#"{"thread_id":"t-2","timestamp":"2026-05-27T10:01:00Z","role":"user","content":"other"}"#,
            r#"{"thread_id":"t-1","timestamp":"2026-05-27T10:02:00Z","role":"assistant","content":"reply"}"#,
        ];
        let f = write_history(&lines);
        let sessions = parse_history(f.path(), None).unwrap();
        assert_eq!(sessions.len(), 2);
        let t1 = sessions.iter().find(|s| s.external_id == "t-1").unwrap();
        assert_eq!(t1.turns.len(), 2);
        assert_eq!(t1.turns[1].role, "assistant");
    }

    #[test]
    fn handles_missing_state_db_gracefully() {
        let lines = [
            r#"{"thread_id":"x","timestamp":"2026-05-27T10:00:00Z","role":"user","content":"hi","model":"gpt-5"}"#,
        ];
        let f = write_history(&lines);
        let bogus_db = std::path::PathBuf::from("/nonexistent/state.sqlite");
        let sessions = parse_history(f.path(), Some(&bogus_db)).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].model.as_deref(), Some("gpt-5"));
    }

    #[test]
    fn enriches_with_state_db_when_available() {
        // Create a real state db with one thread row.
        let db = tempfile::Builder::new()
            .prefix("codex-state-")
            .suffix(".sqlite")
            .tempfile()
            .unwrap();
        {
            let conn = Connection::open(db.path()).unwrap();
            conn.execute_batch(
                "CREATE TABLE threads (id TEXT PRIMARY KEY, title TEXT, project TEXT, created_at TEXT);
                 INSERT INTO threads VALUES ('thread-A','My title','altevra','2026-05-27T09:00:00Z');",
            )
            .unwrap();
        }
        let lines = [
            r#"{"thread_id":"thread-A","timestamp":"2026-05-27T10:00:00Z","role":"user","content":"hi"}"#,
        ];
        let f = write_history(&lines);
        let sessions = parse_history(f.path(), Some(db.path())).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].project_name.as_deref(), Some("altevra"));
        // started_at should come from the db (09:00) not the history (10:00).
        assert_eq!(sessions[0].started_at.format("%H").to_string(), "09");
    }
}

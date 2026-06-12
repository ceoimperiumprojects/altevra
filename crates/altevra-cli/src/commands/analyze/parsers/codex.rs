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

/// A line in `history.jsonl`. Codex uses two different field-name conventions
/// across versions — we accept both via aliases:
///
/// * `thread_id` / `threadId` — identifies the session/thread.
/// * `timestamp` / `ts` — when the turn happened.
/// * `content` / `text` — the turn text.
/// * `session_id` — observed in newer Codex builds as an alias for thread_id.
///
/// The `#[serde(alias)]` machinery normalises all variants into one struct.
#[derive(Debug, Deserialize)]
struct HistoryLine {
    #[serde(
        default,
        alias = "thread_id",
        alias = "threadId",
        alias = "session_id"
    )]
    thread_id: Option<String>,
    #[serde(default, alias = "ts")]
    timestamp: Option<HistoryTimestamp>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default, alias = "text")]
    content: serde_json::Value,
    #[serde(default)]
    tool_name: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

/// `ts` in the REAL `~/.codex/history.jsonl` is a Unix epoch integer
/// (`"ts":1776717927`); older builds wrote `timestamp` as an RFC3339 string.
/// Accept both — an untagged enum lets serde pick whichever shape arrives so
/// no real line is dropped as "malformed".
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum HistoryTimestamp {
    Epoch(i64),
    Text(String),
}

impl HistoryTimestamp {
    fn to_utc(&self) -> Option<DateTime<Utc>> {
        match self {
            // Heuristic: values past ~Nov 2286 in seconds are millisecond
            // epochs (some tools log ms) — divide down before converting.
            HistoryTimestamp::Epoch(v) if *v > 10_000_000_000 => {
                DateTime::from_timestamp(*v / 1000, 0)
            }
            HistoryTimestamp::Epoch(v) => DateTime::from_timestamp(*v, 0),
            HistoryTimestamp::Text(s) => DateTime::parse_from_rfc3339(s)
                .ok()
                .map(|t| t.with_timezone(&Utc)),
        }
    }
}

#[derive(Debug, Default, Clone)]
struct ThreadMetadata {
    /// Reserved — Codex `threads.title` value, surfaced via MCP in v0.5+.
    #[allow(dead_code)]
    title: Option<String>,
    project: Option<String>,
    cwd: Option<String>,
    created_at: Option<DateTime<Utc>>,
}

/// Parse a `created_at` value from `state_5.sqlite` that may arrive in either
/// of two formats:
///
/// * **RFC3339 string** — `"2026-05-27T10:00:00Z"` (stored as TEXT).
/// * **Unix epoch integer** — `1748340000` (stored as INTEGER, seconds since
///   epoch). Observed in newer Codex builds.
///
/// Returns `None` when the column is NULL or unparseable.
fn parse_codex_created_at(raw: Option<rusqlite::types::Value>) -> Option<DateTime<Utc>> {
    use rusqlite::types::Value;
    match raw? {
        Value::Text(s) => DateTime::parse_from_rfc3339(&s)
            .ok()
            .map(|t| t.with_timezone(&Utc)),
        Value::Integer(epoch_secs) => {
            DateTime::from_timestamp(epoch_secs, 0)
        }
        Value::Real(f) => {
            DateTime::from_timestamp(f as i64, 0)
        }
        _ => None,
    }
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
    // R3: spec query is `id, title, cwd, created_at`. The REAL Codex
    // `threads` schema has no `project` column (verified 2026-06-11 against
    // ~/.codex/state_5.sqlite), while older fixtures/builds carry `project`
    // but no `cwd`. Rather than a prepare-fail ladder (a single missing
    // column would discard ALL metadata), select everything and pull the
    // columns we know about by name — missing columns degrade to None
    // per-field instead of per-table.
    let mut stmt = match conn.prepare("SELECT * FROM threads") {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(db = %state_db.display(), error = %e, "codex state db has no threads table");
            return out;
        }
    };
    let rows = stmt.query_map([], |row| {
        let id: String = row.get("id")?;
        let title: Option<String> = row.get("title").ok();
        let project: Option<String> = row.get("project").ok();
        let cwd: Option<String> = row.get("cwd").ok();
        let created_raw: Option<rusqlite::types::Value> = row.get("created_at").ok();
        Ok((id, title, project, cwd, created_raw))
    });
    if let Ok(iter) = rows {
        for r in iter.flatten() {
            let (id, title, project, cwd, created_raw) = r;
            let created_at = parse_codex_created_at(created_raw);
            out.insert(
                id,
                ThreadMetadata {
                    title,
                    project,
                    cwd,
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
            .as_ref()
            .and_then(|t| t.to_utc())
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

        // R3: working_dir comes from threads.cwd in state_5.sqlite. The real
        // schema has no `project` column, so project_name falls back to the
        // cwd basename (same convention as the claude-code parser).
        let working_dir = metadata.get(&thread_id).and_then(|m| m.cwd.clone());
        let project_name = metadata
            .get(&thread_id)
            .and_then(|m| m.project.clone())
            .or_else(|| {
                working_dir
                    .as_deref()
                    .and_then(|p| p.trim_end_matches('/').rsplit('/').next())
                    .filter(|s| !s.is_empty())
                    .map(String::from)
            });
        sessions.push(ImportedSession {
            external_id: thread_id.clone(),
            tool_id: "codex".into(),
            project_name,
            started_at,
            ended_at,
            model,
            turns,
            imported_from: history_path.to_path_buf(),
            working_dir,
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

    // -----------------------------------------------------------------------
    // R3: Codex schema-robustness tests (timestamp variants + real field names)
    // -----------------------------------------------------------------------

    /// Codex `created_at` stored as an RFC3339 string (original format).
    #[test]
    fn state_db_created_at_rfc3339_string() {
        let db = tempfile::Builder::new()
            .prefix("codex-state-rfc3339-")
            .suffix(".sqlite")
            .tempfile()
            .unwrap();
        {
            let conn = Connection::open(db.path()).unwrap();
            conn.execute_batch(
                "CREATE TABLE threads (id TEXT PRIMARY KEY, title TEXT, project TEXT, cwd TEXT, created_at TEXT);
                 INSERT INTO threads VALUES ('t-rfc3339','title','proj','/home/pavle/projekti','2026-05-27T09:00:00Z');",
            )
            .unwrap();
        }
        let lines = [
            r#"{"thread_id":"t-rfc3339","timestamp":"2026-05-27T10:00:00Z","role":"user","content":"hi"}"#,
        ];
        let f = write_history(&lines);
        let sessions = parse_history(f.path(), Some(db.path())).unwrap();
        assert_eq!(sessions.len(), 1);
        // started_at comes from db (09:00), not history (10:00).
        assert_eq!(sessions[0].started_at.format("%H").to_string(), "09");
        // cwd threaded from state db.
        assert_eq!(
            sessions[0].working_dir.as_deref(),
            Some("/home/pavle/projekti")
        );
    }

    /// Codex `created_at` stored as a Unix epoch integer (newer builds).
    #[test]
    fn state_db_created_at_epoch_integer() {
        let db = tempfile::Builder::new()
            .prefix("codex-state-epoch-")
            .suffix(".sqlite")
            .tempfile()
            .unwrap();
        // 2026-05-27T09:00:00Z = 1779872400 seconds since epoch.
        let epoch: i64 = 1779872400;
        {
            let conn = Connection::open(db.path()).unwrap();
            conn.execute_batch(&format!(
                "CREATE TABLE threads (id TEXT PRIMARY KEY, title TEXT, project TEXT, cwd TEXT, created_at INTEGER);
                 INSERT INTO threads VALUES ('t-epoch','title','proj-epoch','/tmp/epoch-cwd',{epoch});"
            ))
            .unwrap();
        }
        let lines = [
            r#"{"thread_id":"t-epoch","timestamp":"2026-05-27T10:00:00Z","role":"user","content":"epoch test"}"#,
        ];
        let f = write_history(&lines);
        let sessions = parse_history(f.path(), Some(db.path())).unwrap();
        assert_eq!(sessions.len(), 1);
        // The epoch 1779872400 is 2026-05-27T09:00:00Z, so started_at hour = 09.
        assert_eq!(
            sessions[0].started_at.format("%H").to_string(),
            "09",
            "epoch integer created_at must parse to the correct time"
        );
        assert_eq!(
            sessions[0].working_dir.as_deref(),
            Some("/tmp/epoch-cwd"),
            "cwd from state_db with epoch timestamp"
        );
    }

    /// history.jsonl with `session_id`/`ts`/`text` field names (newer Codex).
    #[test]
    fn history_jsonl_real_field_names() {
        // Newer Codex builds write `session_id`, `ts`, and `text` instead of
        // `thread_id`, `timestamp`, and `content`.
        let lines = [
            r#"{"session_id":"s-new","ts":"2026-05-27T10:00:00Z","role":"user","text":"using new fields"}"#,
            r#"{"session_id":"s-new","ts":"2026-05-27T10:01:00Z","role":"assistant","text":"response"}"#,
        ];
        let f = write_history(&lines);
        let sessions = parse_history(f.path(), None).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].external_id, "s-new");
        assert_eq!(sessions[0].turns.len(), 2);
        assert_eq!(sessions[0].turns[0].content, "using new fields");
        assert_eq!(sessions[0].turns[1].role, "assistant");
    }

    /// Mixed history.jsonl: both old and new field names in the same file.
    #[test]
    fn history_jsonl_mixed_old_and_new_field_names() {
        let lines = [
            // Old-style
            r#"{"thread_id":"mix-1","timestamp":"2026-05-27T10:00:00Z","role":"user","content":"old style"}"#,
            // New-style
            r#"{"session_id":"mix-2","ts":"2026-05-27T10:00:00Z","role":"user","text":"new style"}"#,
        ];
        let f = write_history(&lines);
        let sessions = parse_history(f.path(), None).unwrap();
        assert_eq!(sessions.len(), 2);
        let mix1 = sessions.iter().find(|s| s.external_id == "mix-1").unwrap();
        assert_eq!(mix1.turns[0].content, "old style");
        let mix2 = sessions.iter().find(|s| s.external_id == "mix-2").unwrap();
        assert_eq!(mix2.turns[0].content, "new style");
    }

    /// parse_codex_created_at: direct unit test of the helper function.
    #[test]
    fn parse_created_at_rfc3339() {
        use rusqlite::types::Value;
        let v = Some(Value::Text("2026-05-27T09:00:00Z".to_string()));
        let dt = parse_codex_created_at(v).unwrap();
        assert_eq!(dt.format("%H").to_string(), "09");
    }

    #[test]
    fn parse_created_at_epoch_int() {
        use rusqlite::types::Value;
        // 1779872400 = 2026-05-27T09:00:00Z
        let v = Some(Value::Integer(1779872400));
        let dt = parse_codex_created_at(v).unwrap();
        assert_eq!(dt.format("%H").to_string(), "09");
    }

    #[test]
    fn parse_created_at_null_returns_none() {
        assert!(parse_codex_created_at(None).is_none());
    }

    /// REAL-SHAPED history.jsonl lines: `session_id` + epoch-int `ts` + `text`
    /// and no `role` field (verified against the live ~/.codex/history.jsonl
    /// on 2026-06-11). These must parse — an Option<String> timestamp would
    /// reject every real line as malformed.
    #[test]
    fn history_jsonl_epoch_int_ts() {
        let lines = [
            // 1779872400 = 2026-05-27T09:00:00Z
            r#"{"session_id":"s-epoch","ts":1779872400,"text":"real shape"}"#,
            r#"{"session_id":"s-epoch","ts":1779872460,"text":"second line"}"#,
        ];
        let f = write_history(&lines);
        let sessions = parse_history(f.path(), None).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].external_id, "s-epoch");
        assert_eq!(sessions[0].turns.len(), 2);
        assert_eq!(sessions[0].turns[0].content, "real shape");
        // role defaults to "user" when absent (history.jsonl logs user input).
        assert_eq!(sessions[0].turns[0].role, "user");
        assert_eq!(
            sessions[0].started_at.format("%H").to_string(),
            "09",
            "epoch-int ts must parse to the correct time"
        );
    }

    /// Millisecond epoch `ts` values are detected and divided down.
    #[test]
    fn history_timestamp_millis_heuristic() {
        let secs = HistoryTimestamp::Epoch(1779872400).to_utc().unwrap();
        let millis = HistoryTimestamp::Epoch(1779872400000).to_utc().unwrap();
        assert_eq!(secs, millis);
    }

    /// REAL threads schema: created_at INTEGER, cwd present, NO `project`
    /// column. Metadata must still load (cwd + created_at) rather than being
    /// discarded wholesale because one optional column is missing.
    #[test]
    fn state_db_real_schema_without_project_column() {
        let db = tempfile::Builder::new()
            .prefix("codex-state-real-")
            .suffix(".sqlite")
            .tempfile()
            .unwrap();
        {
            let conn = Connection::open(db.path()).unwrap();
            conn.execute_batch(
                "CREATE TABLE threads (
                    id TEXT PRIMARY KEY,
                    rollout_path TEXT NOT NULL DEFAULT '',
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL DEFAULT 0,
                    cwd TEXT NOT NULL,
                    title TEXT NOT NULL DEFAULT ''
                 );
                 INSERT INTO threads (id, created_at, cwd, title)
                 VALUES ('t-real', 1779872400, '/home/pavle/projekti/ai-tooling/altevra', 'real thread');",
            )
            .unwrap();
        }
        let lines = [
            r#"{"session_id":"t-real","ts":1779876000,"text":"hello from real codex"}"#,
        ];
        let f = write_history(&lines);
        let sessions = parse_history(f.path(), Some(db.path())).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(
            sessions[0].working_dir.as_deref(),
            Some("/home/pavle/projekti/ai-tooling/altevra"),
            "cwd must survive a schema without the `project` column"
        );
        // project_name falls back to cwd basename.
        assert_eq!(sessions[0].project_name.as_deref(), Some("altevra"));
        // created_at from the epoch-int column (09:00), not history (10:00).
        assert_eq!(sessions[0].started_at.format("%H").to_string(), "09");
    }
}

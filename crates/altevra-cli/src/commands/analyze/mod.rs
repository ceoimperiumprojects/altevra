//! v0.3.8 Analyze Everything — historical session importer.
//!
//! Walks every supported AI tool's local storage (Claude Code JSONL, Codex
//! SQLite, Cursor JSONL, Antigravity JSONL, Hermes JSON), parses sessions
//! into the unified `ImportedSession`/`ImportedTurn` types, and feeds them
//! through `SessionsRepository::upsert_imported` for idempotent storage.
//!
//! Obsidian vault ingest reuses `altevra-vault::scan_vault` +
//! `altevra-memory::ingest_file`. Optional Gemini Flash summaries are produced
//! via `altevra-llm::GeminiFlashChat`.

pub mod discovery;
pub mod orchestrator;
pub mod parsers;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A single turn extracted from a tool-native session file. Content is the
/// raw text we want to record; redaction happens in the orchestrator before
/// it lands in SQLite.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportedTurn {
    /// 0-based monotonic index within the session.
    pub turn_idx: i64,
    /// One of: `user`, `assistant`, `system`, `tool_call`, `tool_result`.
    pub role: String,
    pub content: String,
    /// JSON array describing tool invocations on this turn.
    pub tool_calls: Option<serde_json::Value>,
    /// Denormalized tool name when role is tool_call / tool_result.
    pub tool_name: Option<String>,
    pub model: Option<String>,
    pub tokens_in: Option<i64>,
    pub tokens_out: Option<i64>,
    pub latency_ms: Option<i64>,
    pub created_at: DateTime<Utc>,
}

/// A normalized session extracted by any parser.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportedSession {
    /// Tool-native session id. Required — combined with `tool_id` it is the
    /// idempotency key for `SessionsRepository::upsert_imported`.
    pub external_id: String,
    pub tool_id: String,
    pub project_name: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub model: Option<String>,
    pub turns: Vec<ImportedTurn>,
    pub imported_from: PathBuf,
}

impl ImportedSession {
    pub fn turn_count(&self) -> usize {
        self.turns.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imported_session_counts_turns() {
        let s = ImportedSession {
            external_id: "abc".into(),
            tool_id: "claude-code".into(),
            project_name: None,
            started_at: Utc::now(),
            ended_at: None,
            model: None,
            turns: vec![],
            imported_from: PathBuf::from("/tmp/x.jsonl"),
        };
        assert_eq!(s.turn_count(), 0);
    }
}

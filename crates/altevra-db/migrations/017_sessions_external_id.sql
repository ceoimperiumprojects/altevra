-- v0.3.8 Analyze Everything — idempotent historical session import.
--
-- external_id is the tool-native session identifier (e.g. Claude Code JSONL
-- filename UUID, Codex thread id, Cursor sessionId, Antigravity conversationId,
-- Hermes session id). Combined with the existing `tool` column, it uniquely
-- identifies a session in its source system and lets us detect duplicates on
-- re-run of `altevra setup analyze-everything`.
--
-- imported_from stores the absolute path on disk we read the session from,
-- useful for debugging and incremental rescans.

ALTER TABLE sessions ADD COLUMN external_id TEXT;
ALTER TABLE sessions ADD COLUMN imported_from TEXT;

-- Partial unique index: only enforced when external_id is set, so existing
-- live-recorded sessions (no external_id) remain unconstrained.
CREATE UNIQUE INDEX IF NOT EXISTS idx_sessions_tool_external
    ON sessions (tool, external_id)
    WHERE external_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_sessions_imported_from
    ON sessions (imported_from);

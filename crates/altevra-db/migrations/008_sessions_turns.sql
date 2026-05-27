-- v0.3.1 Omniscient Brain OS — session + turn recorder.
--
-- Sessions are coarse-grained agent work intervals (one Claude Code session,
-- one Codex session, ...). Turns are fine-grained interactions inside a
-- session: user prompts, assistant responses, tool calls and their results.
--
-- Content is stored RAW (Pavle's directive: full content + tool args).
-- Secret redaction happens at the CLI layer before insert (altevra-secrets
-- ::redactor) so what lands on disk is already clean.

CREATE TABLE IF NOT EXISTS sessions (
    id                  TEXT PRIMARY KEY,
    tool                TEXT NOT NULL,                -- claude-code, codex, cursor, antigravity, ...
    project_id          TEXT,
    project_name        TEXT,                          -- denormalised for fast filtering
    started_at          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    ended_at            TEXT,
    summary             TEXT,                          -- generated on session_end
    tokens_in_total     INTEGER NOT NULL DEFAULT 0,
    tokens_out_total    INTEGER NOT NULL DEFAULT 0,
    cost_usd_estimate   REAL NOT NULL DEFAULT 0.0,
    turn_count          INTEGER NOT NULL DEFAULT 0,
    metadata            TEXT NOT NULL DEFAULT '{}'
);

CREATE INDEX IF NOT EXISTS idx_sessions_tool ON sessions (tool);
CREATE INDEX IF NOT EXISTS idx_sessions_project ON sessions (project_id, project_name);
CREATE INDEX IF NOT EXISTS idx_sessions_started ON sessions (started_at DESC);

CREATE TABLE IF NOT EXISTS turns (
    id                  TEXT PRIMARY KEY,
    session_id          TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    turn_idx            INTEGER NOT NULL,              -- monotonic per session
    role                TEXT NOT NULL,                  -- user, assistant, system, tool_call, tool_result
    content             TEXT NOT NULL,                  -- redacted at write time
    tool_calls          TEXT,                           -- JSON array (null for plain messages)
    tool_name           TEXT,                           -- denormalised when role=tool_call/tool_result
    model               TEXT,
    tokens_in           INTEGER,
    tokens_out          INTEGER,
    latency_ms          INTEGER,
    file_changes        TEXT,                           -- JSON array of {path, before_hash, after_hash}
    redacted_count      INTEGER NOT NULL DEFAULT 0,    -- how many secrets were redacted
    created_at          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (session_id, turn_idx)
);

CREATE INDEX IF NOT EXISTS idx_turns_session ON turns (session_id, turn_idx);
CREATE INDEX IF NOT EXISTS idx_turns_role ON turns (role);
CREATE INDEX IF NOT EXISTS idx_turns_tool ON turns (tool_name);
CREATE INDEX IF NOT EXISTS idx_turns_created ON turns (created_at DESC);

-- File change tracking — separate from turn-embedded file_changes JSON so we
-- can query "all changes to README.md" directly without scanning turns.
CREATE TABLE IF NOT EXISTS file_changes (
    id                  TEXT PRIMARY KEY,
    session_id          TEXT REFERENCES sessions(id) ON DELETE SET NULL,
    turn_id             TEXT REFERENCES turns(id) ON DELETE SET NULL,
    path                TEXT NOT NULL,
    before_hash         TEXT,                           -- null on create
    after_hash          TEXT,                           -- null on delete
    diff_summary        TEXT,                           -- +N -M lines, etc.
    actor_type          TEXT NOT NULL,                  -- agent, user, system
    actor_id            TEXT,
    created_at          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_file_changes_path ON file_changes (path, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_file_changes_session ON file_changes (session_id);

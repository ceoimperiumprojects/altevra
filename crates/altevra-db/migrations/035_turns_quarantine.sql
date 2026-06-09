-- P0 `altevra db unify` — divergent-turn quarantine.
--
-- During unify, two turns may collide on (session_id, turn_idx) while their
-- content/tool_calls/file_changes hashes differ (forked or copied shadow DB).
-- The rule (PLAN-ALIVE §P0.2, locked): NEVER overwrite the canonical turn,
-- NEVER violate `turns`' UNIQUE(session_id, turn_idx) — route the divergent
-- row HERE instead, with provenance (which shadow DB it came from) and the
-- collision reason, so Pavle can adjudicate later. Quarantine-not-delete.
--
-- No FK to sessions/turns on purpose: a quarantined row must survive even if
-- the canonical session is later forgotten/deleted (it is evidence).

CREATE TABLE IF NOT EXISTS turns_quarantine (
    id                TEXT PRIMARY KEY,              -- fresh uuid for this quarantine row
    original_turn_id  TEXT NOT NULL,                 -- the shadow turn's own id
    session_id        TEXT NOT NULL,                 -- canonical (post-remap) session id
    turn_idx          INTEGER NOT NULL,
    role              TEXT NOT NULL,
    content           TEXT NOT NULL,                 -- guard-redacted before insert
    tool_calls        TEXT,
    tool_name         TEXT,
    model             TEXT,
    tokens_in         INTEGER,
    tokens_out        INTEGER,
    latency_ms        INTEGER,
    file_changes      TEXT,
    redacted_count    INTEGER NOT NULL DEFAULT 0,
    source_tool       TEXT,
    sensitivity       TEXT NOT NULL DEFAULT 'restricted',
    redaction_status  TEXT NOT NULL DEFAULT 'unscanned',
    created_at        TEXT,
    working_dir       TEXT,
    source_db         TEXT NOT NULL,                 -- shadow DB path this row came from
    reason            TEXT NOT NULL,                 -- divergent_turn_idx | turn_id_collision | ...
    quarantined_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE INDEX IF NOT EXISTS idx_turns_quarantine_session
    ON turns_quarantine (session_id, turn_idx);

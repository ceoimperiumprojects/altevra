-- P0.1 (T1.5/R5): the ephemeral context_packet body + its source list.
-- The packet BODY is ephemeral (auto-purge 14d, R-EPH); the audit of what it
-- exposed lives in exposure_decisions (021) and is never purged (R5-INV).

CREATE TABLE IF NOT EXISTS context_packets (
    id                TEXT PRIMARY KEY,
    schema_version    INTEGER NOT NULL DEFAULT 1,
    compiler_version  TEXT NOT NULL,
    profile_id        TEXT NOT NULL,
    intent            TEXT NOT NULL,
    project           TEXT,
    request           TEXT NOT NULL,            -- echoed RetrievalRequest (JSON)
    db_snapshot       TEXT,
    token_budget      INTEGER NOT NULL DEFAULT 0,
    tokens_used       INTEGER NOT NULL DEFAULT 0,
    truncated         INTEGER NOT NULL DEFAULT 0,
    truncation_reason TEXT,
    items             TEXT NOT NULL DEFAULT '[]',  -- [ContextPacketItem] (JSON)
    excluded          TEXT NOT NULL DEFAULT '[]',  -- [ExclusionRecord] (JSON, capped)
    warnings          TEXT NOT NULL DEFAULT '[]',
    stats             TEXT NOT NULL DEFAULT '{}',
    items_hash        TEXT,                        -- sha256 of items (determinism check)
    exposure_decision_id TEXT,                     -- FK to the durable audit (021)
    sensitivity_ceiling  TEXT NOT NULL DEFAULT 'internal',
    created_at        TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX IF NOT EXISTS idx_context_packets_created ON context_packets(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_context_packets_project ON context_packets(project);

CREATE TABLE IF NOT EXISTS context_packet_sources (
    id            TEXT PRIMARY KEY,
    packet_id     TEXT NOT NULL,
    object_type   TEXT NOT NULL,
    object_id     TEXT NOT NULL,
    object_revision INTEGER,
    rank          INTEGER,
    section       TEXT,
    source_index  TEXT,                          -- tag|bm25|graph|structured
    fused_score   REAL,
    UNIQUE(packet_id, object_type, object_id)
);
CREATE INDEX IF NOT EXISTS idx_packet_sources_packet ON context_packet_sources(packet_id);

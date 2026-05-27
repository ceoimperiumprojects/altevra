-- SQLite-flavored schema.
--   * UUIDs stored as TEXT (36-char dashed form, sqlx-uuid friendly).
--   * Timestamps stored as TEXT (ISO-8601, sqlx-chrono friendly).
--   * JSON columns stored as TEXT (sqlx::types::Json or serde_json::Value via Json wrapper).
--   * BOOLEAN stored as INTEGER (0/1) — sqlx Bool maps cleanly.

CREATE TABLE IF NOT EXISTS events (
    id              TEXT PRIMARY KEY,
    event_type      TEXT NOT NULL,
    project_id      TEXT,
    actor_type      TEXT NOT NULL,
    actor_id        TEXT,
    source          TEXT NOT NULL,
    entity_type     TEXT,
    entity_id       TEXT,
    title           TEXT NOT NULL,
    summary         TEXT,
    payload         TEXT NOT NULL DEFAULT '{}',
    sensitivity     TEXT NOT NULL DEFAULT 'internal',
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    processed_at    TEXT,
    status          TEXT NOT NULL DEFAULT 'pending'
);

CREATE INDEX IF NOT EXISTS idx_events_created_at ON events (created_at DESC);
CREATE INDEX IF NOT EXISTS idx_events_project_id ON events (project_id);
CREATE INDEX IF NOT EXISTS idx_events_event_type ON events (event_type);
CREATE INDEX IF NOT EXISTS idx_events_status ON events (status);

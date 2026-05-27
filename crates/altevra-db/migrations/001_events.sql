CREATE TABLE IF NOT EXISTS events (
    id              UUID PRIMARY KEY,
    event_type      TEXT NOT NULL,
    project_id      UUID,
    actor_type      TEXT NOT NULL,
    actor_id        TEXT,
    source          TEXT NOT NULL,
    entity_type     TEXT,
    entity_id       TEXT,
    title           TEXT NOT NULL,
    summary         TEXT,
    payload         JSONB NOT NULL DEFAULT '{}',
    sensitivity     TEXT NOT NULL DEFAULT 'internal',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    processed_at    TIMESTAMPTZ,
    status          TEXT NOT NULL DEFAULT 'pending'
);

CREATE INDEX IF NOT EXISTS idx_events_created_at ON events (created_at DESC);
CREATE INDEX IF NOT EXISTS idx_events_project_id ON events (project_id);
CREATE INDEX IF NOT EXISTS idx_events_event_type ON events (event_type);
CREATE INDEX IF NOT EXISTS idx_events_status ON events (status);

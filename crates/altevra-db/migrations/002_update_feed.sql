CREATE TABLE IF NOT EXISTS update_feed (
    id                          UUID PRIMARY KEY,
    event_id                    UUID NOT NULL REFERENCES events(id) ON DELETE CASCADE,
    project_id                  UUID,
    update_type                 TEXT NOT NULL,
    importance                  TEXT NOT NULL DEFAULT 'low',
    title                       TEXT NOT NULL,
    short_summary               TEXT NOT NULL,
    agent_summary               TEXT,
    affected_entities           JSONB NOT NULL DEFAULT '[]',
    recommended_agent_action    TEXT,
    visible_to_agents           BOOLEAN NOT NULL DEFAULT TRUE,
    sensitivity                 TEXT NOT NULL DEFAULT 'internal',
    created_at                  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS update_read_state (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    actor_type          TEXT NOT NULL,
    actor_id            TEXT NOT NULL,
    project_id          UUID,
    last_seen_event_id  UUID REFERENCES events(id),
    last_seen_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (actor_type, actor_id, project_id)
);

CREATE INDEX IF NOT EXISTS idx_update_feed_created_at ON update_feed (created_at DESC);
CREATE INDEX IF NOT EXISTS idx_update_feed_project_id ON update_feed (project_id);
CREATE INDEX IF NOT EXISTS idx_update_feed_importance ON update_feed (importance);
CREATE INDEX IF NOT EXISTS idx_update_feed_visible ON update_feed (visible_to_agents);

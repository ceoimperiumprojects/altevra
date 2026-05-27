CREATE TABLE IF NOT EXISTS update_feed (
    id                          TEXT PRIMARY KEY,
    event_id                    TEXT NOT NULL REFERENCES events(id) ON DELETE CASCADE,
    project_id                  TEXT,
    update_type                 TEXT NOT NULL,
    importance                  TEXT NOT NULL DEFAULT 'low',
    title                       TEXT NOT NULL,
    short_summary               TEXT NOT NULL,
    agent_summary               TEXT,
    affected_entities           TEXT NOT NULL DEFAULT '[]',
    recommended_agent_action    TEXT,
    visible_to_agents           INTEGER NOT NULL DEFAULT 1,
    sensitivity                 TEXT NOT NULL DEFAULT 'internal',
    created_at                  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_update_feed_created_at ON update_feed (created_at DESC);
CREATE INDEX IF NOT EXISTS idx_update_feed_project_id ON update_feed (project_id);
CREATE INDEX IF NOT EXISTS idx_update_feed_importance ON update_feed (importance);
CREATE INDEX IF NOT EXISTS idx_update_feed_visible ON update_feed (visible_to_agents);

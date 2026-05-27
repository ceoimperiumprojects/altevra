-- Per-feed fetch state — last fetched timestamp + HTTP cache hints.
CREATE TABLE IF NOT EXISTS research_feed_state (
    feed_id TEXT PRIMARY KEY,
    last_fetched_at TEXT,
    last_etag TEXT,
    last_modified TEXT,
    fail_count INTEGER NOT NULL DEFAULT 0,
    last_error TEXT
);

CREATE INDEX IF NOT EXISTS idx_research_feed_state_last_fetched
    ON research_feed_state(last_fetched_at DESC);

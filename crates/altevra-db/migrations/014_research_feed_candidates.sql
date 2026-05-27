-- Discovery queue for auto-discovered RSS/feed candidates.
-- Brain job FeedDiscovery walks recent research_items + turns, extracts feed
-- links from HTML head, and inserts here. Full-auto mode promotes immediately
-- into the feeds.yaml file (status=promoted). Manual mode leaves status=pending.

CREATE TABLE IF NOT EXISTS research_feed_candidates (
    id TEXT PRIMARY KEY,
    candidate_url TEXT NOT NULL UNIQUE,
    feed_url TEXT,
    source_url TEXT,
    discovered_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    discovered_by TEXT,
    status TEXT NOT NULL DEFAULT 'pending',
    auto_promoted_at TEXT,
    rejected_reason TEXT
);

CREATE INDEX IF NOT EXISTS idx_feed_candidates_status
    ON research_feed_candidates(status, discovered_at DESC);

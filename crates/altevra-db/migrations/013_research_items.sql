-- Research items pulled from RSS/Atom feeds. Dedup by (feed_id, guid).
CREATE TABLE IF NOT EXISTS research_items (
    id TEXT PRIMARY KEY,
    feed_id TEXT NOT NULL,
    guid TEXT NOT NULL,
    link TEXT NOT NULL,
    title TEXT NOT NULL,
    summary TEXT NOT NULL DEFAULT '',
    published_at TEXT,
    ingested_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    relevance_score REAL NOT NULL DEFAULT 0.0,
    project_matches_json TEXT NOT NULL DEFAULT '[]',
    UNIQUE(feed_id, guid)
);

CREATE INDEX IF NOT EXISTS idx_research_items_ingested
    ON research_items(ingested_at DESC);

CREATE INDEX IF NOT EXISTS idx_research_items_relevance
    ON research_items(relevance_score DESC, ingested_at DESC);

CREATE INDEX IF NOT EXISTS idx_research_items_feed
    ON research_items(feed_id, ingested_at DESC);

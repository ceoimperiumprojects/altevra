-- Extend research_items with source_kind so we can tell apart RSS, GitHub
-- trending, web-search, and monitor-page results in queries and briefs.

ALTER TABLE research_items
    ADD COLUMN source_kind TEXT NOT NULL DEFAULT 'rss';

CREATE INDEX IF NOT EXISTS idx_research_items_source_kind
    ON research_items(source_kind, ingested_at DESC);

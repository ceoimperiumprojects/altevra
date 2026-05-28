-- v0.3 Phase 1 — Wiki Layer foundation.
--
-- A wiki page is the synthesized, agent-readable, human-readable record of
-- "what Altevra currently understands about a topic". Pages live on disk
-- as markdown under `wiki/` with typed YAML frontmatter (see
-- `crates/altevra-vault/src/wiki.rs`). This SQLite table is the indexed
-- view that lets us query/list/search without re-parsing every file.
--
-- The `topic` column is UNIQUE — one canonical page per topic. The disk
-- path is recorded for round-trip (open in editor, propose diff, etc.).

CREATE TABLE IF NOT EXISTS wiki_pages (
    id                  TEXT PRIMARY KEY,
    topic               TEXT NOT NULL UNIQUE,
    slug                TEXT NOT NULL,
    path                TEXT NOT NULL,
    status              TEXT NOT NULL DEFAULT 'living',
    confidence          TEXT NOT NULL DEFAULT 'medium',
    sensitivity         TEXT NOT NULL DEFAULT 'internal',
    source_count        INTEGER NOT NULL DEFAULT 0,
    last_synthesized_at TEXT,
    title               TEXT,
    checksum            TEXT NOT NULL DEFAULT '',
    created_at          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE INDEX IF NOT EXISTS idx_wiki_pages_topic ON wiki_pages (topic);
CREATE INDEX IF NOT EXISTS idx_wiki_pages_status ON wiki_pages (status);

-- Edge table for [[wiki-links]] extracted from page bodies. `to_topic` is
-- denormalized intentionally — the target page may not yet exist (a "link
-- to a topic Altevra hasn't synthesized") and we still want to surface
-- those as candidate topics for Wiki Curator in Phase 5.
CREATE TABLE IF NOT EXISTS wiki_page_links (
    id              TEXT PRIMARY KEY,
    from_page_id    TEXT NOT NULL REFERENCES wiki_pages(id) ON DELETE CASCADE,
    to_topic        TEXT NOT NULL,
    link_type       TEXT NOT NULL DEFAULT 'reference',
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE INDEX IF NOT EXISTS idx_wiki_links_from ON wiki_page_links (from_page_id);
CREATE INDEX IF NOT EXISTS idx_wiki_links_to ON wiki_page_links (to_topic);

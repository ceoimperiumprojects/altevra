-- P0.1 (T1.2/T1.3/R8): relations edge table, object_index, and new gap-object
-- tables (learning, insight_card) carrying the full envelope.

-- Canonical edge table (§1.6) — generalizes wiki_page_links.
CREATE TABLE IF NOT EXISTS relations (
    id            TEXT PRIMARY KEY,
    from_type     TEXT NOT NULL,
    from_id       TEXT NOT NULL,
    rel           TEXT NOT NULL,             -- predicate enum (open on read)
    to_type       TEXT,
    to_id         TEXT,
    to_ref        TEXT,                      -- by-key target (wiki topic, url, slug)
    weight        REAL,
    sensitivity   TEXT NOT NULL DEFAULT 'internal',
    provenance    TEXT NOT NULL DEFAULT '{"origin":"system_derived"}',
    status        TEXT NOT NULL DEFAULT 'active',  -- active | retracted
    valid_until   TEXT,
    created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    UNIQUE(from_type, from_id, rel, to_type, to_id, to_ref)
);
CREATE INDEX IF NOT EXISTS idx_relations_from ON relations(from_type, from_id);
CREATE INDEX IF NOT EXISTS idx_relations_to ON relations(to_type, to_id);
CREATE INDEX IF NOT EXISTS idx_relations_rel ON relations(rel);

-- Denormalized cross-type index (R8.3) — one gate point for default-safe reads.
CREATE TABLE IF NOT EXISTS object_index (
    type            TEXT NOT NULL,
    id              TEXT NOT NULL,
    status          TEXT NOT NULL,
    sensitivity     TEXT NOT NULL,
    domain          TEXT NOT NULL,
    scope           TEXT,
    title           TEXT,
    categories      TEXT NOT NULL DEFAULT '[]',
    tags            TEXT NOT NULL DEFAULT '[]',
    updated_at      TEXT NOT NULL,
    PRIMARY KEY (type, id)
);
CREATE INDEX IF NOT EXISTS idx_object_index_domain ON object_index(domain, status);
CREATE INDEX IF NOT EXISTS idx_object_index_sensitivity ON object_index(sensitivity);
CREATE INDEX IF NOT EXISTS idx_object_index_scope ON object_index(scope);
CREATE INDEX IF NOT EXISTS idx_object_index_updated ON object_index(updated_at DESC);

-- learning: a durable learning/lesson object (full envelope).
CREATE TABLE IF NOT EXISTS learnings (
    id              TEXT PRIMARY KEY,
    type            TEXT NOT NULL DEFAULT 'learning',
    schema_version  INTEGER NOT NULL DEFAULT 1,
    title           TEXT NOT NULL,
    body            TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'active',
    domain          TEXT NOT NULL DEFAULT 'business',
    scope           TEXT,
    sensitivity     TEXT NOT NULL DEFAULT 'internal',
    provenance      TEXT NOT NULL DEFAULT '{"origin":"pavle_direct"}',
    redaction_status TEXT NOT NULL DEFAULT 'unscanned',
    revision        INTEGER NOT NULL DEFAULT 1,
    tags            TEXT NOT NULL DEFAULT '[]',
    categories      TEXT NOT NULL DEFAULT '[]',
    risk_tags       TEXT NOT NULL DEFAULT '[]',
    confidence      TEXT NOT NULL DEFAULT 'medium',
    source_path     TEXT,
    checksum        TEXT,
    supersedes      TEXT,
    superseded_by   TEXT,
    valid_until     TEXT,
    review_after    TEXT,
    origin_device   TEXT,
    policy_version  INTEGER,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX IF NOT EXISTS idx_learnings_domain ON learnings(domain, status);

-- insight_card: DB-only synthesized insight (full envelope, promotable to wiki).
CREATE TABLE IF NOT EXISTS insight_cards (
    id              TEXT PRIMARY KEY,
    type            TEXT NOT NULL DEFAULT 'insight_card',
    schema_version  INTEGER NOT NULL DEFAULT 1,
    title           TEXT NOT NULL,
    body            TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'active',
    domain          TEXT NOT NULL DEFAULT 'business',
    scope           TEXT,
    sensitivity     TEXT NOT NULL DEFAULT 'internal',
    provenance      TEXT NOT NULL DEFAULT '{"origin":"agent_inferred"}',
    redaction_status TEXT NOT NULL DEFAULT 'unscanned',
    revision        INTEGER NOT NULL DEFAULT 1,
    tags            TEXT NOT NULL DEFAULT '[]',
    categories      TEXT NOT NULL DEFAULT '[]',
    risk_tags       TEXT NOT NULL DEFAULT '[]',
    confidence      TEXT NOT NULL DEFAULT 'medium',
    supersedes      TEXT,
    superseded_by   TEXT,
    valid_until     TEXT,
    review_after    TEXT,
    origin_device   TEXT,
    policy_version  INTEGER,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX IF NOT EXISTS idx_insight_cards_domain ON insight_cards(domain, status);

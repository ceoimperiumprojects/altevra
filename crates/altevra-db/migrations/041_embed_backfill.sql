-- R2: Embedding backfill state (watermark + model meta). Used by
-- `altevra embed backfill` to track which DB objects have already been
-- enqueued so re-runs are idempotent without re-scanning every table.
--
-- embed_backfill_watermark: one row per (source_type, model) pair.
-- `last_id` is the highest primary-key (UUID as TEXT, lexicographic)
-- processed so far — the backfill resumes from there.
--
-- embed_meta: records which model+dim is "the active embedder" for this
-- DB. Written on first embed. Only one model should be active at a time;
-- the write-gate in vector_store uses this to refuse mismatched dims.

CREATE TABLE IF NOT EXISTS embed_backfill_watermark (
    id          TEXT PRIMARY KEY,   -- UUID
    source_type TEXT NOT NULL,      -- "turn" | "learning" | "note" | "wiki" | "research"
    model       TEXT NOT NULL,
    last_id     TEXT NOT NULL DEFAULT '',    -- last processed object id (lexicographic)
    total_enqueued INTEGER NOT NULL DEFAULT 0,
    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    UNIQUE(source_type, model)
);

-- Active embedding model meta. At most ONE row per model name.
-- dim is the vector dimension produced by that model.
CREATE TABLE IF NOT EXISTS embed_meta (
    model       TEXT PRIMARY KEY,
    dim         INTEGER NOT NULL,
    set_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE INDEX IF NOT EXISTS idx_embed_backfill_type ON embed_backfill_watermark(source_type);

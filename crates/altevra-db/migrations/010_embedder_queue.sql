-- v0.3.3 continuous embedder queue. Drained by altevra-memory::worker which
-- calls the configured embedding provider (Gemini text-embedding-004 by
-- default) and writes resulting vectors into memory_chunk_vectors.

CREATE TABLE IF NOT EXISTS embedder_queue (
    chunk_id        TEXT PRIMARY KEY REFERENCES memory_chunks(id) ON DELETE CASCADE,
    status          TEXT NOT NULL DEFAULT 'pending',  -- pending|in_progress|done|failed
    enqueued_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    started_at      TEXT,
    finished_at     TEXT,
    fail_count      INTEGER NOT NULL DEFAULT 0,
    last_error      TEXT
);

CREATE INDEX IF NOT EXISTS idx_embedder_queue_status ON embedder_queue (status, enqueued_at);

-- Plain-TEXT vector storage (sqlite-vec is optional feature). The embedding
-- column holds a JSON-encoded Vec<f32>. Naive cosine in Rust handles up to
-- a few hundred thousand chunks before we need ANN.
CREATE TABLE IF NOT EXISTS memory_chunk_vectors_v2 (
    chunk_id        TEXT PRIMARY KEY,
    model           TEXT NOT NULL,
    dim             INTEGER NOT NULL,
    embedding       TEXT NOT NULL,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_chunk_vectors_model ON memory_chunk_vectors_v2 (model);

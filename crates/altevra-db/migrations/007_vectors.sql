-- Vector storage for memory chunks.
--
-- This file always creates a plain TEXT-backed fallback table
-- (`memory_chunk_vectors`) so the schema is identical with or without the
-- `vec` feature. The TEXT column stores a JSON-encoded `Vec<f32>` array;
-- callers compute cosine in Rust (BM25 + naive cosine fallback) until
-- sqlite-vec is wired in.
--
-- When the `vec` cargo feature is enabled, `pool::register_vec_extension`
-- additionally registers the sqlite-vec extension at connection time and the
-- runtime will create a `vec_memory_chunks` virtual table (vec0) lazily, since
-- `CREATE VIRTUAL TABLE` cannot live inside a migration file that must also
-- work without the extension loaded.

CREATE TABLE IF NOT EXISTS memory_chunk_vectors (
    chunk_id   TEXT PRIMARY KEY REFERENCES memory_chunks(id) ON DELETE CASCADE,
    dim        INTEGER NOT NULL,
    embedding  TEXT NOT NULL,    -- JSON-encoded Vec<f32>
    model      TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_memory_chunk_vectors_dim ON memory_chunk_vectors (dim);

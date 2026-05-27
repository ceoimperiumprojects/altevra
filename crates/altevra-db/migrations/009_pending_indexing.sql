-- v0.3.2 watcher queue: files that need (re-)indexing into memory_chunks.
-- The continuous embedder (v0.3.3) drains this queue.

CREATE TABLE IF NOT EXISTS pending_indexing (
    id              TEXT PRIMARY KEY,
    path            TEXT NOT NULL,
    queued_at       TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    status          TEXT NOT NULL DEFAULT 'pending',  -- pending|in_progress|done|failed
    last_attempt_at TEXT,
    error           TEXT,
    fail_count      INTEGER NOT NULL DEFAULT 0,
    UNIQUE (path)
);

CREATE INDEX IF NOT EXISTS idx_pending_status ON pending_indexing (status);
CREATE INDEX IF NOT EXISTS idx_pending_queued ON pending_indexing (queued_at);

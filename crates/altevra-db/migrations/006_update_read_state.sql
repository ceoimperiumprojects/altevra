-- Note: an `update_read_state` table is also declared in 002_update_feed.sql in
-- the original Postgres schema. Here we keep the "canonical" definition (the
-- Postgres v2 used a 006 redefinition without the events FK, since the FK was
-- relaxed for the SQLite/JSON state world). To stay idempotent under SQLite we
-- only create-if-not-exists; the table is identical in both files modulo the
-- (since-removed) FK. SQLite does not allow ALTER TABLE ADD CONSTRAINT, but
-- since 002 already created the table with the same shape this is a no-op.

CREATE TABLE IF NOT EXISTS update_read_state (
    id                  TEXT PRIMARY KEY,
    actor_type          TEXT NOT NULL,
    actor_id            TEXT NOT NULL,
    project_id          TEXT,
    last_seen_event_id  TEXT,
    last_seen_at        TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (actor_type, actor_id, project_id)
);

CREATE INDEX IF NOT EXISTS idx_read_state_actor ON update_read_state (actor_type, actor_id);
CREATE INDEX IF NOT EXISTS idx_read_state_project ON update_read_state (project_id);

CREATE TABLE IF NOT EXISTS tasks (
    id                  TEXT PRIMARY KEY,
    project_id          TEXT,
    title               TEXT NOT NULL,
    description         TEXT,
    status              TEXT NOT NULL DEFAULT 'open',
    priority            TEXT NOT NULL DEFAULT 'medium',
    assignee            TEXT,
    due_at              TEXT,
    metadata            TEXT NOT NULL DEFAULT '{}',
    created_at          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_tasks_project ON tasks (project_id);
CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks (status);

CREATE TABLE IF NOT EXISTS goals (
    id                  TEXT PRIMARY KEY,
    project_id          TEXT,
    title               TEXT NOT NULL,
    description         TEXT,
    target_date         TEXT,
    status              TEXT NOT NULL DEFAULT 'active',
    metadata            TEXT NOT NULL DEFAULT '{}',
    created_at          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE IF NOT EXISTS decisions (
    id                  TEXT PRIMARY KEY,
    project_id          TEXT,
    title               TEXT NOT NULL,
    rationale           TEXT,
    decided_at          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    decided_by          TEXT,
    metadata            TEXT NOT NULL DEFAULT '{}'
);

CREATE TABLE IF NOT EXISTS review_items (
    id                  TEXT PRIMARY KEY,
    project_id          TEXT,
    kind                TEXT NOT NULL,
    title               TEXT NOT NULL,
    body                TEXT,
    status              TEXT NOT NULL DEFAULT 'open',
    created_at          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    metadata            TEXT NOT NULL DEFAULT '{}'
);

CREATE TABLE IF NOT EXISTS memory_documents (
    id                  TEXT PRIMARY KEY,
    project_id          TEXT,
    source_path         TEXT NOT NULL,
    title               TEXT,
    body                TEXT NOT NULL,
    checksum            TEXT NOT NULL,
    metadata            TEXT NOT NULL DEFAULT '{}',
    indexed_at          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    UNIQUE (source_path)
);

CREATE TABLE IF NOT EXISTS memory_chunks (
    id                  TEXT PRIMARY KEY,
    document_id         TEXT NOT NULL REFERENCES memory_documents(id) ON DELETE CASCADE,
    heading_path        TEXT,
    text                TEXT NOT NULL,
    checksum            TEXT NOT NULL,
    start_byte          INTEGER NOT NULL,
    end_byte            INTEGER NOT NULL,
    created_at          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_memory_chunks_doc ON memory_chunks (document_id);

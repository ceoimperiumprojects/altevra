CREATE TABLE IF NOT EXISTS update_read_state (
    id                  UUID PRIMARY KEY,
    actor_type          TEXT NOT NULL,
    actor_id            TEXT NOT NULL,
    project_id          UUID,
    last_seen_event_id  UUID,
    last_seen_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (actor_type, actor_id, project_id)
);

CREATE INDEX IF NOT EXISTS idx_read_state_actor ON update_read_state (actor_type, actor_id);
CREATE INDEX IF NOT EXISTS idx_read_state_project ON update_read_state (project_id);

CREATE TABLE IF NOT EXISTS tasks (
    id                  UUID PRIMARY KEY,
    project_id          UUID,
    title               TEXT NOT NULL,
    description         TEXT,
    status              TEXT NOT NULL DEFAULT 'open',
    priority            TEXT NOT NULL DEFAULT 'medium',
    assignee            TEXT,
    due_at              TIMESTAMPTZ,
    metadata            JSONB NOT NULL DEFAULT '{}',
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_tasks_project ON tasks (project_id);
CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks (status);

CREATE TABLE IF NOT EXISTS goals (
    id                  UUID PRIMARY KEY,
    project_id          UUID,
    title               TEXT NOT NULL,
    description         TEXT,
    target_date         DATE,
    status              TEXT NOT NULL DEFAULT 'active',
    metadata            JSONB NOT NULL DEFAULT '{}',
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS decisions (
    id                  UUID PRIMARY KEY,
    project_id          UUID,
    title               TEXT NOT NULL,
    rationale           TEXT,
    decided_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    decided_by          TEXT,
    metadata            JSONB NOT NULL DEFAULT '{}'
);

CREATE TABLE IF NOT EXISTS review_items (
    id                  UUID PRIMARY KEY,
    project_id          UUID,
    kind                TEXT NOT NULL,
    title               TEXT NOT NULL,
    body                TEXT,
    status              TEXT NOT NULL DEFAULT 'open',
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    metadata            JSONB NOT NULL DEFAULT '{}'
);

CREATE TABLE IF NOT EXISTS memory_documents (
    id                  UUID PRIMARY KEY,
    project_id          UUID,
    source_path         TEXT NOT NULL,
    title               TEXT,
    body                TEXT NOT NULL,
    checksum            TEXT NOT NULL,
    metadata            JSONB NOT NULL DEFAULT '{}',
    indexed_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (source_path)
);

CREATE TABLE IF NOT EXISTS memory_chunks (
    id                  UUID PRIMARY KEY,
    document_id         UUID NOT NULL REFERENCES memory_documents(id) ON DELETE CASCADE,
    heading_path        TEXT,
    text                TEXT NOT NULL,
    checksum            TEXT NOT NULL,
    start_byte          INT NOT NULL,
    end_byte            INT NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_memory_chunks_doc ON memory_chunks (document_id);

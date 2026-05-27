CREATE TABLE IF NOT EXISTS hooks (
    id          UUID PRIMARY KEY,
    slug        TEXT NOT NULL UNIQUE,
    version     TEXT NOT NULL,
    source_file TEXT NOT NULL,
    checksum    TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'active',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS hook_runs (
    id              UUID PRIMARY KEY,
    hook_slug       TEXT NOT NULL,
    tool_name       TEXT NOT NULL,
    project_id      UUID,
    payload         JSONB NOT NULL DEFAULT '{}',
    result          JSONB NOT NULL DEFAULT '{}',
    success         BOOLEAN NOT NULL DEFAULT FALSE,
    error_message   TEXT,
    duration_ms     BIGINT NOT NULL DEFAULT 0,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_hooks_slug ON hooks (slug);
CREATE INDEX IF NOT EXISTS idx_hook_runs_slug ON hook_runs (hook_slug);
CREATE INDEX IF NOT EXISTS idx_hook_runs_created_at ON hook_runs (created_at DESC);
CREATE INDEX IF NOT EXISTS idx_hook_runs_tool ON hook_runs (tool_name);

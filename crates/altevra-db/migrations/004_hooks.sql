CREATE TABLE IF NOT EXISTS hooks (
    id          TEXT PRIMARY KEY,
    slug        TEXT NOT NULL UNIQUE,
    version     TEXT NOT NULL,
    source_file TEXT NOT NULL,
    checksum    TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'active',
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE IF NOT EXISTS hook_runs (
    id              TEXT PRIMARY KEY,
    hook_slug       TEXT NOT NULL,
    tool_name       TEXT NOT NULL,
    project_id      TEXT,
    payload         TEXT NOT NULL DEFAULT '{}',
    result          TEXT NOT NULL DEFAULT '{}',
    success         INTEGER NOT NULL DEFAULT 0,
    error_message   TEXT,
    duration_ms     INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_hooks_slug ON hooks (slug);
CREATE INDEX IF NOT EXISTS idx_hook_runs_slug ON hook_runs (hook_slug);
CREATE INDEX IF NOT EXISTS idx_hook_runs_created_at ON hook_runs (created_at DESC);
CREATE INDEX IF NOT EXISTS idx_hook_runs_tool ON hook_runs (tool_name);

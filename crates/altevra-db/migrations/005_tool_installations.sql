CREATE TABLE IF NOT EXISTS tool_installations (
    id                  TEXT PRIMARY KEY,
    tool_name           TEXT NOT NULL,
    project_id          TEXT,
    adapter_version     TEXT NOT NULL,
    installed_at        TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    last_verified_at    TEXT,
    status              TEXT NOT NULL DEFAULT 'active',
    metadata            TEXT NOT NULL DEFAULT '{}',
    UNIQUE (tool_name, project_id)
);

CREATE TABLE IF NOT EXISTS installed_components (
    id                  TEXT PRIMARY KEY,
    installation_id     TEXT NOT NULL REFERENCES tool_installations(id) ON DELETE CASCADE,
    component_type      TEXT NOT NULL,
    component_slug      TEXT NOT NULL,
    installed_version   TEXT NOT NULL,
    installed_path      TEXT NOT NULL,
    checksum            TEXT NOT NULL,
    status              TEXT NOT NULL DEFAULT 'current',
    last_checked_at     TEXT,
    UNIQUE (installation_id, component_slug)
);

CREATE INDEX IF NOT EXISTS idx_tool_inst_tool_name ON tool_installations (tool_name);
CREATE INDEX IF NOT EXISTS idx_installed_comp_inst ON installed_components (installation_id);
CREATE INDEX IF NOT EXISTS idx_installed_comp_status ON installed_components (status);

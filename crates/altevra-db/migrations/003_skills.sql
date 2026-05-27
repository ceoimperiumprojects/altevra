CREATE TABLE IF NOT EXISTS skills (
    id          TEXT PRIMARY KEY,
    slug        TEXT NOT NULL UNIQUE,
    version     TEXT NOT NULL,
    source_path TEXT NOT NULL,
    checksum    TEXT NOT NULL,
    content     TEXT NOT NULL,
    metadata    TEXT NOT NULL DEFAULT '{}',
    status      TEXT NOT NULL DEFAULT 'active',
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE IF NOT EXISTS skill_installations (
    id                  TEXT PRIMARY KEY,
    skill_slug          TEXT NOT NULL REFERENCES skills(slug) ON DELETE CASCADE,
    tool_name           TEXT NOT NULL,
    project_id          TEXT,
    installed_version   TEXT NOT NULL,
    installed_at        TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    last_verified_at    TEXT,
    checksum            TEXT NOT NULL,
    status              TEXT NOT NULL DEFAULT 'current',
    UNIQUE (skill_slug, tool_name, project_id)
);

CREATE INDEX IF NOT EXISTS idx_skills_slug ON skills (slug);
CREATE INDEX IF NOT EXISTS idx_skill_installations_tool ON skill_installations (tool_name);

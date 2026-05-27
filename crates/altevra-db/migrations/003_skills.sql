CREATE TABLE IF NOT EXISTS skills (
    id          UUID PRIMARY KEY,
    slug        TEXT NOT NULL UNIQUE,
    version     TEXT NOT NULL,
    source_path TEXT NOT NULL,
    checksum    TEXT NOT NULL,
    content     TEXT NOT NULL,
    metadata    JSONB NOT NULL DEFAULT '{}',
    status      TEXT NOT NULL DEFAULT 'active',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS skill_installations (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    skill_slug          TEXT NOT NULL REFERENCES skills(slug) ON DELETE CASCADE,
    tool_name           TEXT NOT NULL,
    project_id          UUID,
    installed_version   TEXT NOT NULL,
    installed_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_verified_at    TIMESTAMPTZ,
    checksum            TEXT NOT NULL,
    status              TEXT NOT NULL DEFAULT 'current',
    UNIQUE (skill_slug, tool_name, project_id)
);

CREATE INDEX IF NOT EXISTS idx_skills_slug ON skills (slug);
CREATE INDEX IF NOT EXISTS idx_skill_installations_tool ON skill_installations (tool_name);

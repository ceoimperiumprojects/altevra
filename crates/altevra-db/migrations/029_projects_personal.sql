-- P0.8 personal-brain + project tables (working draft §6, R7). `learnings` and
-- `insight_cards` already exist (020); these complete the personal data types so
-- Altevra holds personal life with the same dignity as business (CLAUDE.md §3.1).
-- High-water domains default Restricted/local-only (the policy in 024 governs).

CREATE TABLE IF NOT EXISTS projects (
    id             TEXT PRIMARY KEY,
    name           TEXT NOT NULL,
    parent_id      TEXT,                       -- R7 scope hierarchy (project→parent→global)
    status         TEXT NOT NULL DEFAULT 'active',
    domain         TEXT NOT NULL DEFAULT 'project',
    sensitivity    TEXT NOT NULL DEFAULT 'internal',
    scope          TEXT,
    schema_version INTEGER NOT NULL DEFAULT 1,
    provenance     TEXT NOT NULL DEFAULT '{"origin":"imported_readonly"}',
    tags           TEXT NOT NULL DEFAULT '[]',
    categories     TEXT NOT NULL DEFAULT '["project"]',
    redaction_status TEXT NOT NULL DEFAULT 'clean',
    created_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at     TEXT
);
CREATE INDEX IF NOT EXISTS idx_projects_parent ON projects(parent_id);

CREATE TABLE IF NOT EXISTS persons (
    id             TEXT PRIMARY KEY,
    name           TEXT NOT NULL,
    note           TEXT,
    domain         TEXT NOT NULL DEFAULT 'relationship',
    sensitivity    TEXT NOT NULL DEFAULT 'restricted',
    status         TEXT NOT NULL DEFAULT 'active',
    schema_version INTEGER NOT NULL DEFAULT 1,
    provenance     TEXT NOT NULL DEFAULT '{"origin":"pavle_direct"}',
    tags           TEXT NOT NULL DEFAULT '[]',
    categories     TEXT NOT NULL DEFAULT '["person"]',
    redaction_status TEXT NOT NULL DEFAULT 'clean',
    created_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at     TEXT
);

CREATE TABLE IF NOT EXISTS relationships (
    id             TEXT PRIMARY KEY,
    person_id      TEXT NOT NULL,
    kind           TEXT NOT NULL,              -- family|partner|friend|mentor|colleague|...
    note           TEXT,
    domain         TEXT NOT NULL DEFAULT 'relationship',
    sensitivity    TEXT NOT NULL DEFAULT 'restricted',
    status         TEXT NOT NULL DEFAULT 'active',
    schema_version INTEGER NOT NULL DEFAULT 1,
    provenance     TEXT NOT NULL DEFAULT '{"origin":"pavle_direct"}',
    redaction_status TEXT NOT NULL DEFAULT 'clean',
    created_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX IF NOT EXISTS idx_relationships_person ON relationships(person_id);

CREATE TABLE IF NOT EXISTS preferences (
    id             TEXT PRIMARY KEY,
    pref_key       TEXT NOT NULL,
    pref_value     TEXT NOT NULL,
    domain         TEXT NOT NULL DEFAULT 'personal',
    sensitivity    TEXT NOT NULL DEFAULT 'confidential',
    status         TEXT NOT NULL DEFAULT 'active',
    schema_version INTEGER NOT NULL DEFAULT 1,
    provenance     TEXT NOT NULL DEFAULT '{"origin":"pavle_direct"}',
    redaction_status TEXT NOT NULL DEFAULT 'clean',
    created_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at     TEXT
);
CREATE INDEX IF NOT EXISTS idx_preferences_key ON preferences(pref_key);

CREATE TABLE IF NOT EXISTS event_log_personal (
    id             TEXT PRIMARY KEY,
    summary        TEXT NOT NULL,
    occurred_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    domain         TEXT NOT NULL DEFAULT 'personal',
    sensitivity    TEXT NOT NULL DEFAULT 'restricted',
    schema_version INTEGER NOT NULL DEFAULT 1,
    provenance     TEXT NOT NULL DEFAULT '{"origin":"pavle_direct"}',
    tags           TEXT NOT NULL DEFAULT '[]',
    categories     TEXT NOT NULL DEFAULT '["event"]',
    redaction_status TEXT NOT NULL DEFAULT 'clean',
    created_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

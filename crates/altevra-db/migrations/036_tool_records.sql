-- P1 (PLAN-ALIVE §P1.1): tool_records — the Tool Register.
--
-- Invocable TOOLS (skills, CLIs, binaries, MCP servers, web services, ADB
-- surfaces, python APIs) are a DISTINCT concept from AI-agent adapters
-- (023 adapter_dossiers). The same name legitimately exists across kinds —
-- "codex" is a skill AND a binary — so uniqueness is UNIQUE(name, kind),
-- NEVER name alone.
--
-- adapter_ref is a link-BY-NAME to adapter_dossiers.tool_name for entities
-- that live in both worlds (hermes / codex / cursor). No SQL FK on purpose:
-- either side may be seeded first. Precedence is defined at the read surface
-- (`get_capabilities`): adapter_dossiers wins for agent-identity fields,
-- tool_records wins for invocation.
--
-- All structured fields follow the JSON-as-TEXT idiom used since 001.
-- SECURITY (PLAN-ALIVE §P1.3): every field passes guard at upsert in
-- ToolRecordsRepository — capability YAMLs / documented invocations routinely
-- embed bearer tokens; a raw credential here would fan out (DB → SessionStart
-- injection → re-recorded into turns → served over MCP).

CREATE TABLE IF NOT EXISTS tool_records (
    id               TEXT PRIMARY KEY,
    type             TEXT NOT NULL DEFAULT 'tool_record',
    schema_version   INTEGER NOT NULL DEFAULT 1,
    name             TEXT NOT NULL,
    kind             TEXT NOT NULL,                  -- skill|cli|python-api|mcp-server|web-service|adb|binary
    display_name     TEXT,
    description      TEXT,
    invocation       TEXT NOT NULL DEFAULT '{}',     -- JSON {canonical, alternates[]}
    locations        TEXT NOT NULL DEFAULT '[]',     -- JSON [path, ...] — ALL discovered installs
    can_do           TEXT NOT NULL DEFAULT '[]',     -- JSON honest capability list
    cannot_do        TEXT NOT NULL DEFAULT '[]',     -- JSON
    unverified       TEXT NOT NULL DEFAULT '[]',     -- JSON
    requires_session TEXT NOT NULL DEFAULT '{}',     -- JSON (login/session prerequisites)
    status           TEXT NOT NULL DEFAULT 'unverified', -- can|cannot|unverified
    last_verified_at TEXT,
    categories       TEXT NOT NULL DEFAULT '["tool"]',
    source           TEXT NOT NULL DEFAULT 'scan',   -- scan|hook|manual
    adapter_ref      TEXT,                           -- adapter_dossiers.tool_name (link-by-name)
    created_at       TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at       TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    UNIQUE(name, kind)
);

CREATE INDEX IF NOT EXISTS idx_tool_records_kind ON tool_records(kind);
CREATE INDEX IF NOT EXISTS idx_tool_records_status ON tool_records(status);

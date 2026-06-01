-- P0.2 (BUILD_TASKS T2.1, §5, R9/R10): capability/tool registry gap objects.
-- adapter_dossier, capability_record, skill_proposal, capability_grant — each
-- with the full envelope. installed_component gains the typed CapabilityState
-- columns (component_type=skill folds in skill_installations per R10).

-- adapter_dossier: per-tool capability matrix as a durable object.
CREATE TABLE IF NOT EXISTS adapter_dossiers (
    id              TEXT PRIMARY KEY,
    type            TEXT NOT NULL DEFAULT 'adapter_dossier',
    schema_version  INTEGER NOT NULL DEFAULT 1,
    tool_name       TEXT NOT NULL UNIQUE,           -- claude-code|codex|cursor|antigravity|hermes
    adapter_version TEXT NOT NULL,
    support_tier    TEXT NOT NULL DEFAULT 'unverified', -- native|partial|fallback_only|unsupported
    surfaces        TEXT NOT NULL DEFAULT '{}',      -- per-surface SurfaceSupport (JSON)
    hook_events_supported TEXT NOT NULL DEFAULT '[]',
    skill_format    TEXT,                            -- md|mdc|yaml|none
    install_targets TEXT NOT NULL DEFAULT '[]',
    fallback_strategy TEXT,
    detection       TEXT,
    status          TEXT NOT NULL DEFAULT 'active',
    domain          TEXT NOT NULL DEFAULT 'business',
    sensitivity     TEXT NOT NULL DEFAULT 'internal',
    provenance      TEXT NOT NULL DEFAULT '{"origin":"system_derived"}',
    categories      TEXT NOT NULL DEFAULT '["capability"]',
    revision        INTEGER NOT NULL DEFAULT 1,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

-- capability_record: the honest can/cannot/unverified ledger (Law 6, T7).
CREATE TABLE IF NOT EXISTS capability_records (
    id              TEXT PRIMARY KEY,
    type            TEXT NOT NULL DEFAULT 'capability_record',
    schema_version  INTEGER NOT NULL DEFAULT 1,
    actor           TEXT NOT NULL,                  -- altevra|claude-code|codex|...
    capability_key  TEXT NOT NULL,                  -- hook.session_start|mcp.tools|skill.render|...
    support         TEXT NOT NULL DEFAULT 'unverified', -- supported|unsupported|unverified|fallback
    evidence_ref    TEXT,                           -- REQUIRED when support='supported' (T7)
    verification_method TEXT,                        -- tested|declared|observed
    verified_at     TEXT,
    degraded_to     TEXT,
    domain          TEXT NOT NULL DEFAULT 'business',
    sensitivity     TEXT NOT NULL DEFAULT 'internal',
    provenance      TEXT NOT NULL DEFAULT '{"origin":"system_derived"}',
    categories      TEXT NOT NULL DEFAULT '["capability"]',
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    UNIQUE(actor, capability_key)
);

-- skill_proposal: skill-factory output (co-owned §4 gen / §5 render).
CREATE TABLE IF NOT EXISTS skill_proposals (
    id              TEXT PRIMARY KEY,
    type            TEXT NOT NULL DEFAULT 'skill_proposal',
    schema_version  INTEGER NOT NULL DEFAULT 1,
    dedup_hash      TEXT NOT NULL UNIQUE,           -- same workflow proposes once
    proposed_slug   TEXT NOT NULL,
    proposed_body   TEXT NOT NULL DEFAULT '{}',     -- SkillBody (JSON)
    workflow_evidence TEXT NOT NULL DEFAULT '[]',
    occurrences     INTEGER NOT NULL DEFAULT 1,
    target_agents   TEXT NOT NULL DEFAULT '[]',
    capability_grade TEXT NOT NULL DEFAULT 'read',  -- read|propose|render|install|execute
    render_target   TEXT,
    status          TEXT NOT NULL DEFAULT 'proposed', -- proposed|approved|applied|rejected|withdrawn|deprecated
    domain          TEXT NOT NULL DEFAULT 'business',
    sensitivity     TEXT NOT NULL DEFAULT 'internal',
    provenance      TEXT NOT NULL DEFAULT '{"origin":"agent_inferred"}',
    categories      TEXT NOT NULL DEFAULT '["skill"]',
    revision        INTEGER NOT NULL DEFAULT 1,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

-- capability_grant: cross-agent grant (Constitution §10, T9).
CREATE TABLE IF NOT EXISTS capability_grants (
    id              TEXT PRIMARY KEY,
    type            TEXT NOT NULL DEFAULT 'capability_grant',
    schema_version  INTEGER NOT NULL DEFAULT 1,
    grantee         TEXT NOT NULL,                  -- agent/tool receiving the grant
    subject_kind    TEXT NOT NULL,                  -- skill|capability
    subject_ref     TEXT NOT NULL,                  -- slug|capability_key
    trust_level     TEXT NOT NULL DEFAULT 'none',
    requires_approval INTEGER NOT NULL DEFAULT 1,
    approval_ref    TEXT,                           -- review_item that approved (when required)
    scope           TEXT,                           -- project_id | null=global
    status          TEXT NOT NULL DEFAULT 'pending', -- pending|granted|revoked
    granted_at      TEXT,
    expires_at      TEXT,
    domain          TEXT NOT NULL DEFAULT 'business',
    sensitivity     TEXT NOT NULL DEFAULT 'internal',
    provenance      TEXT NOT NULL DEFAULT '{"origin":"system_derived"}',
    categories      TEXT NOT NULL DEFAULT '["capability"]',
    revision        INTEGER NOT NULL DEFAULT 1,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    UNIQUE(grantee, subject_kind, subject_ref)
);

CREATE INDEX IF NOT EXISTS idx_capability_records_actor ON capability_records(actor);
CREATE INDEX IF NOT EXISTS idx_skill_proposals_status ON skill_proposals(status);
CREATE INDEX IF NOT EXISTS idx_capability_grants_grantee ON capability_grants(grantee, status);

-- installed_component gains the typed CapabilityState support (T8: computed by verify).
ALTER TABLE installed_components ADD COLUMN capability_state TEXT NOT NULL DEFAULT 'current';
ALTER TABLE installed_components ADD COLUMN last_verified_at TEXT;

-- P0.1 (T1.4/R5): safety substrate — secret sightings, the append-only audit
-- log, and the exposure-decision audit (separate from the ephemeral packet, R5).
-- NONE of these ever store a raw secret value — only fingerprints/metadata.

-- secret_sighting: a detection record. NEVER the value (§1.10/§2.5).
CREATE TABLE IF NOT EXISTS secret_sightings (
    id              TEXT PRIMARY KEY,
    schema_version  INTEGER NOT NULL DEFAULT 1,
    secret_kind     TEXT NOT NULL,             -- openai|anthropic|aws|github|jwt|pem|db_url|...
    fingerprint     TEXT NOT NULL,             -- sha256[:12] of the value, for dedup/audit
    source_ref      TEXT,                      -- where it was seen (object/turn/file ref)
    location        TEXT,                      -- field/path within the source
    action          TEXT NOT NULL,             -- redacted|rejected|quarantined|granted
    sensitivity     TEXT NOT NULL DEFAULT 'secret',
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    UNIQUE(fingerprint, source_ref)
);
CREATE INDEX IF NOT EXISTS idx_secret_sightings_kind ON secret_sightings(secret_kind);

-- audit_log: append-only, tamper-evident-ready (prev_hash for P1 chaining).
CREATE TABLE IF NOT EXISTS audit_log (
    id              TEXT PRIMARY KEY,
    action          TEXT NOT NULL,             -- redaction_applied|exposure_decision|review_*|forget_*|...
    subject_type    TEXT,
    subject_id      TEXT,
    actor           TEXT NOT NULL,             -- agent:claude-code | user:pavle | system
    details         TEXT NOT NULL DEFAULT '{}',-- redacted metadata, NEVER a secret value
    prev_hash       TEXT,                      -- hash chain (P1); null in P0
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX IF NOT EXISTS idx_audit_log_subject ON audit_log(subject_type, subject_id);
CREATE INDEX IF NOT EXISTS idx_audit_log_action ON audit_log(action, created_at DESC);

-- exposure_decision: the durable, append-only "why was X exposed" record (R5).
-- This is NEVER auto-purged (unlike the ephemeral context_packet body).
CREATE TABLE IF NOT EXISTS exposure_decisions (
    id                TEXT PRIMARY KEY,
    packet_id         TEXT,                    -- the context_packet this decided for (nullable)
    request           TEXT NOT NULL,           -- echoed RetrievalRequest (JSON)
    sensitivity_ceiling TEXT NOT NULL,
    domain_scope      TEXT NOT NULL DEFAULT '[]',
    included_refs     TEXT NOT NULL DEFAULT '[]',  -- [{type,id,rank,reason}]
    excluded_refs     TEXT NOT NULL DEFAULT '[]',  -- [{type,id,reason}] (capped; counts exact)
    redaction_counts  TEXT NOT NULL DEFAULT '{}',
    db_snapshot       TEXT,
    created_at        TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
CREATE INDEX IF NOT EXISTS idx_exposure_decisions_packet ON exposure_decisions(packet_id);
CREATE INDEX IF NOT EXISTS idx_exposure_decisions_created ON exposure_decisions(created_at DESC);

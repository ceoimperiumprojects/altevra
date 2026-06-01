-- P0.8 (BUILD_TASKS T8.1, §6.4): domain_policy durable object + seed the 9
-- builtin domains with the canonical per-domain policy matrix. This is the map
-- §1.12/§2.15/§2.14 defer to: default sensitivity, audience ceiling, cloud-sync
-- ceiling, embedding role, Obsidian mirror, retention class, soft/hard TTL,
-- RTBF, legal-hold. High-water domains (personal/relationship/health/legal/
-- financial/client) default to local-only + no plaintext mirror (D4).

CREATE TABLE IF NOT EXISTS domain_policies (
    id                       TEXT PRIMARY KEY,
    type                     TEXT NOT NULL DEFAULT 'domain_policy',
    schema_version           INTEGER NOT NULL DEFAULT 1,
    domain_key               TEXT NOT NULL UNIQUE,    -- governed enum (§1.5)
    display_name             TEXT NOT NULL,
    is_builtin               INTEGER NOT NULL DEFAULT 1,
    policy_version           INTEGER NOT NULL DEFAULT 1,
    default_sensitivity      TEXT NOT NULL,
    max_sensitivity          TEXT NOT NULL,
    default_audience_ceiling TEXT NOT NULL,
    cloud_sync               TEXT NOT NULL,           -- disabled|encrypted_only|allowed
    embedding_model_role     TEXT NOT NULL,           -- local_private|cloud_ok
    obsidian_mirror          TEXT NOT NULL,           -- never|opt_in|default_on
    obsidian_zone            TEXT,
    retention_class          TEXT NOT NULL,           -- permanent|long|standard|ephemeral
    soft_ttl_days            INTEGER,
    hard_expiry_days         INTEGER,
    review_on_write          INTEGER NOT NULL DEFAULT 0,
    rtbf_required            INTEGER NOT NULL DEFAULT 0,
    legal_hold_capable       INTEGER NOT NULL DEFAULT 0,
    export_class             TEXT NOT NULL DEFAULT 'on_request',
    status                   TEXT NOT NULL DEFAULT 'active',
    sensitivity              TEXT NOT NULL DEFAULT 'internal',
    provenance               TEXT NOT NULL DEFAULT '{"origin":"system_derived"}',
    domain                   TEXT NOT NULL DEFAULT 'business',
    categories               TEXT NOT NULL DEFAULT '["policy"]',
    created_at               TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at               TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

-- Seed the 9 builtins (§6.4 matrix). INSERT OR IGNORE keeps migration idempotent.
INSERT OR IGNORE INTO domain_policies
 (id, domain_key, display_name, default_sensitivity, max_sensitivity, default_audience_ceiling,
  cloud_sync, embedding_model_role, obsidian_mirror, obsidian_zone, retention_class,
  soft_ttl_days, hard_expiry_days, review_on_write, rtbf_required, legal_hold_capable, export_class)
VALUES
 ('dp_business','business','Business','internal','confidential','project_agents','encrypted_only','cloud_ok','opt_in','30-business','long',180,NULL,0,0,0,'on_request'),
 ('dp_project','project','Project','internal','confidential','project_agents','encrypted_only','cloud_ok','default_on','31-projects','standard',90,NULL,0,0,0,'on_request'),
 ('dp_client','client','Client','confidential','restricted','trusted_agents','disabled','local_private','never',NULL,'long',365,NULL,1,1,1,'restricted'),
 ('dp_personal','personal','Personal','confidential','restricted','pavle_only','disabled','local_private','opt_in',NULL,'permanent',NULL,NULL,1,1,0,'restricted'),
 ('dp_relationship','relationship','Relationship','restricted','restricted','pavle_only','disabled','local_private','never',NULL,'permanent',NULL,NULL,1,1,0,'restricted'),
 ('dp_health','health','Health','restricted','restricted','pavle_only','disabled','local_private','never',NULL,'permanent',NULL,NULL,1,1,0,'restricted'),
 ('dp_legal','legal','Legal','confidential','restricted','pavle_only','disabled','local_private','never',NULL,'permanent',NULL,NULL,1,1,1,'restricted'),
 ('dp_financial','financial','Financial','confidential','restricted','pavle_only','disabled','local_private','never',NULL,'long',NULL,2555,1,1,1,'restricted'),
 ('dp_public','public','Public','public','shareable','shareable_public','allowed','cloud_ok','default_on','20-wiki','standard',365,NULL,0,0,0,'always');

CREATE INDEX IF NOT EXISTS idx_domain_policies_key ON domain_policies(domain_key);

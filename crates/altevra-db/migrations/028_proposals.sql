-- P0.6 self-improvement + runaway firewall (working draft §4, R10).
-- ONE unified proposal table with a `kind` discriminator (R10 §4.19#3) — skill
-- proposals (023) remain their own legacy table; this is the super-family for
-- memory/wiki/prompt/category/improvement proposals the resident modes emit.

CREATE TABLE IF NOT EXISTS proposals (
    id              TEXT PRIMARY KEY,
    kind            TEXT NOT NULL,             -- memory|wiki|prompt|category|skill|improvement|...
    risk_tier      TEXT NOT NULL,             -- tier0|tier1|tier2 (SI-9 derived, not asserted)
    status          TEXT NOT NULL DEFAULT 'proposed', -- proposed|triaged|approved|applied|rejected|superseded|withdrawn|deprecated
    title           TEXT NOT NULL,
    body            TEXT NOT NULL,
    source_mode     TEXT,                      -- resident mode that produced it
    dedup_hash      TEXT NOT NULL,             -- SI-13 dedup
    evidence_count  INTEGER NOT NULL DEFAULT 0,
    evidence_refs   TEXT NOT NULL DEFAULT '[]',
    decided_by      TEXT,                      -- set by core AFTER a presence check (HP-2)
    decided_at      TEXT,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_proposals_dedup ON proposals (dedup_hash);
CREATE INDEX IF NOT EXISTS idx_proposals_status ON proposals (status, kind);

-- raw signals the self-improve loop clusters into proposals (stage 1-2).
CREATE TABLE IF NOT EXISTS improvement_signals (
    id              TEXT PRIMARY KEY,
    kind            TEXT NOT NULL,
    source_ref      TEXT NOT NULL,             -- turn/session/run that emitted it
    summary         TEXT NOT NULL,
    cluster_key     TEXT,                      -- groups signals into one proposal
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE INDEX IF NOT EXISTS idx_signals_cluster ON improvement_signals (cluster_key);

-- prompt registry-of-record + rollback (§4.8). `locked` prompts (safety,
-- altevra_rules) are constitutional — no proposal may rewrite them (SI-2).
CREATE TABLE IF NOT EXISTS prompts (
    name            TEXT NOT NULL,
    version         INTEGER NOT NULL,
    layer           TEXT NOT NULL,             -- safety|altevra_rules|mode|task
    body            TEXT NOT NULL,
    locked          INTEGER NOT NULL DEFAULT 0, -- 1 = constitutional, never auto-changed
    active          INTEGER NOT NULL DEFAULT 1,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    PRIMARY KEY (name, version)
);

-- shadow A/B eval results gating prompt changes (SI-10).
CREATE TABLE IF NOT EXISTS prompt_eval_results (
    id              TEXT PRIMARY KEY,
    prompt_name     TEXT NOT NULL,
    candidate_version INTEGER NOT NULL,
    baseline_version  INTEGER NOT NULL,
    score_delta     REAL NOT NULL,             -- candidate - baseline; must be >= gate to apply
    passed          INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

-- seed the two constitutional-locked prompt layers (SI-2).
INSERT OR IGNORE INTO prompts (name, version, layer, body, locked) VALUES
  ('safety',        1, 'safety',        'Never leak secrets/PII; respect sensitivity ceilings; no external side effects without authorization.', 1),
  ('altevra_rules', 1, 'altevra_rules', 'Operate under Altevra; proposals only, never auto-apply Tier-1/2; human presence gates all approvals.', 1);

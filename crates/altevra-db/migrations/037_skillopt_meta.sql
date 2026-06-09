-- P3a (PLAN-ALIVE §P3): skillopt_meta — the SkillOpt optimizer's cross-run
-- edit memory (port of Hivemind's skillopt-meta.jsonl, SQLite-first).
--
-- Each row records that a specific EDIT SET (identified by an
-- order-independent sha256 fingerprint of the canonicalized edit JSONs) was
-- tried against a skill. The proposer checks `was_tried(skill_slug,
-- fingerprint)` BEFORE publishing/queueing, so a failed or already-tried edit
-- set is never re-proposed — this is what stops the optimize loop from
-- churning the same edit forever.
--
-- outcome ladder: proposed → applied | reverted | rejected.
-- UNIQUE(skill_slug, fingerprint): re-recording the same set updates the
-- outcome + tried_at instead of duplicating.

CREATE TABLE IF NOT EXISTS skillopt_meta (
    id          TEXT PRIMARY KEY,
    skill_slug  TEXT NOT NULL,
    fingerprint TEXT NOT NULL,                  -- sha256 hex, order-independent over the edit set
    ops         TEXT NOT NULL DEFAULT '[]',     -- JSON array of short per-edit summaries (NEVER full bodies)
    outcome     TEXT NOT NULL DEFAULT 'proposed', -- proposed|applied|reverted|rejected
    tried_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    UNIQUE(skill_slug, fingerprint)
);

CREATE INDEX IF NOT EXISTS idx_skillopt_meta_slug ON skillopt_meta(skill_slug);

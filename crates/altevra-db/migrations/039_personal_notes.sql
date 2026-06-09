-- P5 personal brain layer (PLAN-ALIVE §P5.1) — EXTENDS migration 029, never
-- migrates away from it (LOCKED). Kinds that already have canonical stores are
-- FK-POINTERS, not parallel rows:
--   person / relationship / preference → 029 persons / relationships / preferences
--   decision / goal → the object-envelope (decisions + object_index) and
--                     goals.json stores that P2's SessionStart injection reads
-- ONLY the net-new kinds live here:
--   place | idea | mood | health | memory | reference | habit | routine |
--   value | identity_shift | life_event
--
-- High-water defaults follow 029/024: personal-domain content defaults
-- Confidential (dp_personal), health/relationship Restricted — the repository
-- raises sensitivity to the domain policy's default at insert and forces
-- review_required for high-water (local_private) domains. Every body passes
-- guard_text at the persistence boundary (PersonalNotesRepository).

CREATE TABLE IF NOT EXISTS personal_notes (
    id               TEXT PRIMARY KEY,
    kind             TEXT NOT NULL,              -- place|idea|mood|health|memory|reference|habit|routine|value|identity_shift|life_event
    body             TEXT NOT NULL,
    domain           TEXT NOT NULL DEFAULT 'personal',
    sensitivity      TEXT NOT NULL DEFAULT 'confidential',
    review_required  INTEGER NOT NULL DEFAULT 0, -- trust ladder: 1 for high-water domains
    status           TEXT NOT NULL DEFAULT 'active',
    schema_version   INTEGER NOT NULL DEFAULT 1,
    provenance       TEXT NOT NULL DEFAULT '{"origin":"pavle_direct"}',
    tags             TEXT NOT NULL DEFAULT '[]',
    categories       TEXT NOT NULL DEFAULT '["personal_note"]',
    redaction_status TEXT NOT NULL DEFAULT 'unscanned',
    person_id        TEXT,                       -- optional link → persons.id (029)
    project_id       TEXT,                       -- optional link → projects.id (029)
    created_at       TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at       TEXT
);
CREATE INDEX IF NOT EXISTS idx_personal_notes_kind ON personal_notes(kind);
CREATE INDEX IF NOT EXISTS idx_personal_notes_domain ON personal_notes(domain);
CREATE INDEX IF NOT EXISTS idx_personal_notes_person ON personal_notes(person_id);

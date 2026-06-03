-- P0.9 E1 — lifecycle archiver scaffolding.
--
-- Adds two non-breaking columns to `object_index` so the lifecycle job can do
-- its safe work without ever hard-deleting:
--
--  * `legal_hold` — per-object active hold flag (D7). A held row is never
--    purged and never archived by the sweep. Domain-level capability lives in
--    `domain_policies.legal_hold_capable` (migration 024); this column is the
--    *active* per-row signal a sweep must consult.
--  * `lifecycle_marker` — soft marker the sweep sets on `delete_due` rows so
--    Pavle's daily digest can surface them. The destructive forget itself
--    remains presence-gated (R4) and runs through the existing `altevra
--    control forget` path — this column never licenses a delete.
--
-- Neither column is referenced by `exposure_decisions` or `audit_log` — those
-- append-only tables stay untouched (R5-INV).

ALTER TABLE object_index ADD COLUMN legal_hold INTEGER NOT NULL DEFAULT 0;
ALTER TABLE object_index ADD COLUMN lifecycle_marker TEXT;

CREATE INDEX IF NOT EXISTS idx_object_index_legal_hold ON object_index(legal_hold);
CREATE INDEX IF NOT EXISTS idx_object_index_lifecycle_marker ON object_index(lifecycle_marker);

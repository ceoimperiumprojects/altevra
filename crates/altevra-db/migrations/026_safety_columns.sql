-- P0.1 / R11 safety hardening: persist the redaction + sensitivity verdict so
-- the read/exposure path can fail-closed.
--
-- Findings (cross-engine R11):
--  * exposure_gate was fail-OPEN for object_index candidates because the index
--    carried no redaction_status (gate received None and skipped the check).
--  * turns persisted only an integer redacted_count — the computed sensitivity
--    and redaction_status were thrown away, so turn reads could not fail-closed.
--
-- Defaults are fail-closed (default-UP): unknown rows are 'unscanned' (never
-- exposable) and 'restricted' (top of the ladder) until a real write overwrites
-- them with the guard's actual verdict.

-- object_index: the packet compiler's candidate source must carry the verdict.
ALTER TABLE object_index ADD COLUMN redaction_status TEXT NOT NULL DEFAULT 'unscanned';

-- turns: persist the guard's verdict alongside the (existing) redacted_count.
ALTER TABLE turns ADD COLUMN sensitivity TEXT NOT NULL DEFAULT 'restricted';
ALTER TABLE turns ADD COLUMN redaction_status TEXT NOT NULL DEFAULT 'unscanned';

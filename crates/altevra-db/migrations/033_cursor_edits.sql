-- P0.9 E4 — Cursor CLI ai-tracking import surface.
--
-- Cursor CLI persists its AI-generated code hashes + tracked file content in
-- ~/.cursor/ai-tracking/ai-code-tracking.db. The Altevra Cursor CLI importer
-- reads that database READ-ONLY (rusqlite with SQLITE_OPEN_READ_ONLY) and lifts
-- the rows into this table after running each indexable text field through
-- guard_text (so credential-class secrets are REJECTED and never persisted,
-- and PII/secret leakage is redacted before the row lands).
--
-- The table follows the standard envelope (status / domain / sensitivity /
-- provenance / redaction_status / categories / tags) so every CursorEdit
-- becomes a first-class object in object_index + object_fts via
-- ObjectIndexRepository::index_object — recall + the packet compiler find
-- them through the existing path, ExposureGate still gates downstream
-- exposure.
--
-- Invariants honored:
--   * SI-7 — high-water content stays local; never auto-mirrored to cloud.
--   * R5-INV — exposure_decisions + audit_log unaffected (we do not touch
--     either; only cursor_edits + object_index + object_fts are written).
--   * R11 — redaction_status is REQUIRED; the writer only indexes scanned
--     verdicts (clean / redacted); rejected rows are dropped (never stored).
--   * TAG-1 — every cursor_edit carries at least its domain category, and a
--     `kind:cursor_edit` tag so the source is filterable.
--   * Read-only on the real Cursor db — this migration creates nothing on the
--     external db; it only allocates the destination table on the Altevra db.

CREATE TABLE IF NOT EXISTS cursor_edits (
    id              TEXT PRIMARY KEY,           -- cursor-edit-<content_hash>
    content_hash    TEXT NOT NULL,              -- the Cursor row's `hash` column
    source          TEXT,                       -- Cursor `source` column (cli / extension / …)
    file_path       TEXT,                       -- Cursor `fileName` column
    file_extension  TEXT,                       -- Cursor `fileExtension` column
    conversation_id TEXT,                       -- Cursor `conversationId` column
    request_id      TEXT,                       -- Cursor `requestId` column
    model           TEXT,                       -- Cursor `model` column
    snippet         TEXT,                       -- guarded body (may be empty if hash-only)
    length          INTEGER,                    -- snippet length in bytes (0 if hash-only)
    cursor_ts       INTEGER,                    -- Cursor `timestamp` column (ms since epoch)
    cursor_created  INTEGER NOT NULL,           -- Cursor `createdAt` column (ms since epoch)
    -- Altevra envelope ----------------------------------------------------
    title           TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'active',
    domain          TEXT NOT NULL DEFAULT 'business',
    scope           TEXT,
    sensitivity     TEXT NOT NULL DEFAULT 'internal',
    provenance      TEXT NOT NULL,              -- JSON {origin, imported_from, source_db, …}
    redaction_status TEXT NOT NULL,             -- guard_text verdict: clean | redacted | …
    categories      TEXT NOT NULL,              -- JSON array
    tags            TEXT NOT NULL,              -- JSON array
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_cursor_edits_content_hash
    ON cursor_edits(content_hash);
CREATE INDEX IF NOT EXISTS idx_cursor_edits_file_path
    ON cursor_edits(file_path);
CREATE INDEX IF NOT EXISTS idx_cursor_edits_cursor_ts
    ON cursor_edits(cursor_ts);

-- P3 install/sync safety rails (PLAN-ALIVE §P3 install/sync) — managed_writes
-- manifest: the stored baseline that makes "never overwrite human edits"
-- DETECTABLE. Every file the skill-sync writer touches gets one row recording
-- the sha256 of the block it wrote (block_hash) and where the pre-write backup
-- landed. On the next sync, current-file hash ≠ manifest hash ⇒ DRIFT ⇒ the
-- writer REFUSES and routes to review instead of clobbering a human edit.
--
-- target_path is UNIQUE: the manifest tracks the LATEST write per file;
-- history lives in the backup tree (~/.altevra/backups/sync/<ts>/).

CREATE TABLE IF NOT EXISTS managed_writes (
    id          TEXT PRIMARY KEY,
    target_path TEXT NOT NULL UNIQUE,
    block_hash  TEXT NOT NULL,                 -- sha256 hex of the content WE wrote
    backup_path TEXT,                          -- pre-write backup of the previous content (NULL on create)
    ts          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE INDEX IF NOT EXISTS idx_managed_writes_ts ON managed_writes(ts DESC);

-- R5: Block-level managed-writes manifest — tracks ALTEVRA_MANAGED_START/END
-- blocks written into human-owned files (e.g. CLAUDE.md).
--
-- Unlike `managed_writes` which is whole-file, each row here corresponds to ONE
-- block identified by (file_path, marker_id) where marker_id is the optional
-- label that may appear in the START marker comment.  The `block_hash` is the
-- sha256 of the bytes between (and including) the START and END comment lines.
-- On the next write: if current block hash != manifest hash → DRIFT → refuse +
-- route to review, never clobber.
--
-- The `provenance` column is a JSON object recording {source_file, mtime,
-- ingest_ts} so we can trace back where each ingested item came from.

CREATE TABLE IF NOT EXISTS block_writes (
    id          TEXT PRIMARY KEY,
    file_path   TEXT NOT NULL,        -- absolute path of the target file
    marker_id   TEXT NOT NULL DEFAULT '',  -- optional label in the START marker ('' = unlabeled)
    block_hash  TEXT NOT NULL,        -- sha256 hex of the block bytes WE wrote
    backup_path TEXT,                 -- pre-write backup location (NULL on first create)
    provenance  TEXT,                 -- JSON metadata (source_file, mtime, ingest_ts)
    ts          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    UNIQUE(file_path, marker_id)
);

CREATE INDEX IF NOT EXISTS idx_block_writes_file ON block_writes(file_path);
CREATE INDEX IF NOT EXISTS idx_block_writes_ts   ON block_writes(ts DESC);

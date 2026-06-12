-- E2: DB-size-trend history — the `db_optimize` weekly brain job records one
-- snapshot of the database's on-disk size after PRAGMA optimize +
-- incremental_vacuum. The doctor's DB-size-trend check reads the two most
-- recent rows and warns when the file is growing faster than a sane threshold
-- (raw turns are NEVER deleted — growth is expected; the check flags ANOMALOUS
-- growth, e.g. a runaway table, not normal accumulation).
--
-- This is metrics-only: it never holds content, only an integer byte size + a
-- timestamp + the freed-bytes the vacuum reclaimed. Append-only; old rows are
-- pruned by the job itself to a bounded window (no unbounded growth from the
-- size tracker that watches for growth).

CREATE TABLE IF NOT EXISTS db_size_history (
    id            TEXT PRIMARY KEY,
    -- on-disk size of the main DB file in bytes at snapshot time.
    size_bytes    INTEGER NOT NULL,
    -- bytes the incremental_vacuum reclaimed this run (0 if WAL/auto_vacuum off).
    freed_bytes   INTEGER NOT NULL DEFAULT 0,
    -- count of brain_jobs that ran in the trailing window (retention-job liveness).
    jobs_in_window INTEGER NOT NULL DEFAULT 0,
    ts            TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE INDEX IF NOT EXISTS idx_db_size_history_ts ON db_size_history(ts DESC);

-- v0.3.4 brain daemon job history. Each periodic job records its run here
-- so `altevra brain jobs --failed` and `altevra brain status` can report.

CREATE TABLE IF NOT EXISTS brain_jobs (
    id              TEXT PRIMARY KEY,
    kind            TEXT NOT NULL,             -- event_classifier|observer_scan|vault_indexer|insight_synthesizer|research_fetcher|daily_summary|task_grooming
    status          TEXT NOT NULL,             -- running|done|failed
    started_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    finished_at     TEXT,
    duration_ms     INTEGER,
    error           TEXT,
    result_summary  TEXT
);

CREATE INDEX IF NOT EXISTS idx_brain_jobs_kind ON brain_jobs (kind, started_at DESC);
CREATE INDEX IF NOT EXISTS idx_brain_jobs_status ON brain_jobs (status);

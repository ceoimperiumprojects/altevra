-- Per-project research agent state — tracks daily budgets and the latest
-- leverage summary so daily briefs can be assembled on demand without
-- re-running scrapes.

CREATE TABLE IF NOT EXISTS project_research_state (
    project_id TEXT PRIMARY KEY,
    last_run_at TEXT,
    queries_used_today INTEGER NOT NULL DEFAULT 0,
    daily_budget INTEGER NOT NULL DEFAULT 5,
    last_leverage_summary_md TEXT,
    last_leverage_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_project_research_state_last_run
    ON project_research_state(last_run_at DESC);

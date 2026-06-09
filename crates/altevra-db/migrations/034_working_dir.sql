-- S0 item 5 — `working_dir` on sessions AND turns.
--
-- Captures the absolute path of the directory from which the agent session was
-- started (or the turn was recorded). This is the *project root* — not the
-- file being edited. Pavle's "run from ~, project elsewhere" case means a turn
-- can record its own cwd even when it differs from the session-level cwd.
--
-- Capture order at hook time:
--   1. $CLAUDE_PROJECT_DIR environment variable (set by Claude Code ≥1.x).
--   2. std::env::current_dir() at the moment the hook fires.
--   3. NULL if both are unavailable.
--
-- Backfill: existing turns inherit their session's working_dir where the
-- session value is non-NULL. Hermes-imported sessions have no cwd context and
-- remain NULL.

ALTER TABLE sessions ADD COLUMN working_dir TEXT;
ALTER TABLE turns    ADD COLUMN working_dir TEXT;

-- Backfill: turns without working_dir inherit from their session.
-- In practice all existing sessions have NULL working_dir (pre-034), so
-- this is a no-op on the initial migration. It runs clean and is idempotent.
UPDATE turns
SET    working_dir = (
    SELECT s.working_dir
    FROM   sessions s
    WHERE  s.id = turns.session_id
)
WHERE  turns.working_dir IS NULL;

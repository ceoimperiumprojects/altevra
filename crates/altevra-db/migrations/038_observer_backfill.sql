-- P4 — observer cold-start backfill watermark (PLAN-ALIVE §P4 #3).
--
-- `altevra observer backfill` synthesizes METADATA-ONLY events from the
-- existing turns/sessions corpus (counts, turn/session IDs as refs, tool
-- names — NEVER turn body content). Idempotency rests on three legs:
--   1. deterministic event ids (UUIDv5 of (source_id, event_type)),
--   2. INSERT OR IGNORE on the events table,
--   3. this watermark row — records how far the corpus has been swept so a
--      re-run only considers rows newer than the watermark.
--
-- Backfilled events carry HISTORICAL timestamps (from the source turns), so
-- they are invisible to the rolling `list_since` windows until an explicit
-- one-shot `altevra observer scan --since @<epoch>` surfaces the cold-start
-- insights. A now()-stamped backfill would flood every 7-day window.

CREATE TABLE IF NOT EXISTS observer_backfill_state (
    id                 TEXT PRIMARY KEY CHECK (id = 'singleton'),
    -- newest source-row created_at swept on the last run (RFC-3339 text;
    -- compared lexicographically like every other timestamp column).
    watermark          TEXT NOT NULL,
    -- oldest synthetic event timestamp ever produced (for the scan hint).
    earliest_event_at  TEXT,
    events_inserted    INTEGER NOT NULL DEFAULT 0,
    runs               INTEGER NOT NULL DEFAULT 0,
    last_run_at        TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

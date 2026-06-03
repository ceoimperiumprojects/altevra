-- C2: SelfImproveOrchestrator firewall-state persistence (working draft §4.7).
-- STAGE 6 (MONITOR) of the self-improve loop must persist the firewall counters
-- so the circuit breaker (SI-11) + Tier-0 daily cap (SI-12) + per-window run
-- budget actually ACCUMULATE across runs — otherwise every run starts from a
-- clean FirewallState and the brakes never engage. One row per window key
-- (e.g. the local day) holds the deltas the orchestrator increments each run.
--
-- The LIMITS the firewall reads still come from `resident_budgets` (027); this
-- table is only the mutable counter state the firewall mutates over time.

CREATE TABLE IF NOT EXISTS resident_firewall_state (
    window_key            TEXT PRIMARY KEY,        -- e.g. 'YYYY-MM-DD' local day
    runs_in_window        INTEGER NOT NULL DEFAULT 0,
    auto_applies_in_window INTEGER NOT NULL DEFAULT 0,
    consecutive_failures  INTEGER NOT NULL DEFAULT 0,
    updated_at            TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

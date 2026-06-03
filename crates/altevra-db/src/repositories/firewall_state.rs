//! Firewall-state persistence (migration 031, self-improve STAGE 6).
//!
//! The runaway [`firewall_check`](altevra_core::selfimprove::firewall_check) is a
//! PURE function of [`FirewallLimits`] + [`FirewallState`]. The LIMITS are static
//! config (read from `resident_budgets`, migration 027); the STATE is mutable
//! counters that must ACCUMULATE across orchestrator runs — otherwise the circuit
//! breaker (SI-11), the per-window run budget, and the Tier-0 daily cap (SI-12)
//! would reset to zero every run and never engage.
//!
//! This repo is the persistence seam for those counters, keyed by a `window_key`
//! (the local day). It loads the live counts into a [`FirewallState`] the
//! orchestrator hands to the firewall, then persists the deltas after the run.
//!
//! It is BELOW the LLM: only integer counters are stored; no free text, no
//! proposal CONTENT can reach this table, so nothing the loop emits can move a
//! counter except the orchestrator's own structured increments (SI-15).

use altevra_core::selfimprove::{FirewallLimits, FirewallState};
use sqlx::{Row, SqlitePool};

pub struct FirewallStateRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> FirewallStateRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Load the accumulated [`FirewallState`] for `window_key` (zeros if the
    /// window has no row yet). `kill_switch` is NOT persisted here — it is a live
    /// flag the orchestrator sets from the RESIDENT_DISABLED check, never from the
    /// DB (a stored kill-switch could be silently cleared; the live check can't).
    pub async fn load(&self, window_key: &str) -> anyhow::Result<FirewallState> {
        let row = sqlx::query(
            "SELECT runs_in_window, auto_applies_in_window, consecutive_failures \
             FROM resident_firewall_state WHERE window_key = ?",
        )
        .bind(window_key)
        .fetch_optional(self.pool)
        .await?;
        Ok(match row {
            Some(r) => FirewallState {
                runs_in_window: r.get::<i64, _>("runs_in_window") as u32,
                auto_applies_in_window: r.get::<i64, _>("auto_applies_in_window") as u32,
                consecutive_failures: r.get::<i64, _>("consecutive_failures") as u32,
                kill_switch: false,
            },
            None => FirewallState::default(),
        })
    }

    /// Persist the accumulated counters for `window_key` (upsert). The orchestrator
    /// calls this AFTER a run with the state it carried + its deltas applied, so the
    /// next run sees the higher water mark (SI-11/SI-12 accumulate). `kill_switch`
    /// is never written (see [`load`](Self::load)).
    pub async fn save(&self, window_key: &str, state: &FirewallState) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO resident_firewall_state \
             (window_key, runs_in_window, auto_applies_in_window, consecutive_failures, updated_at) \
             VALUES (?, ?, ?, ?, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')) \
             ON CONFLICT(window_key) DO UPDATE SET \
               runs_in_window = excluded.runs_in_window, \
               auto_applies_in_window = excluded.auto_applies_in_window, \
               consecutive_failures = excluded.consecutive_failures, \
               updated_at = excluded.updated_at",
        )
        .bind(window_key)
        .bind(state.runs_in_window as i64)
        .bind(state.auto_applies_in_window as i64)
        .bind(state.consecutive_failures as i64)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Load the firewall LIMITS from `resident_budgets` for a mode (defaults when the
    /// mode has no row). `resident_budgets` is per-DAY (`max_runs_per_day`), so the
    /// window the orchestrator keys on is the local day. `max_auto_applies` and the
    /// circuit-breaker threshold are not yet first-class columns there, so we keep
    /// the core defaults for those — the run budget is the one that comes from config.
    pub async fn limits_for(&self, mode: &str) -> anyhow::Result<FirewallLimits> {
        let row = sqlx::query("SELECT max_runs_per_day FROM resident_budgets WHERE mode = ?")
            .bind(mode)
            .fetch_optional(self.pool)
            .await?;
        let mut limits = FirewallLimits::default();
        if let Some(r) = row {
            limits.max_runs_per_window = r.get::<i64, _>("max_runs_per_day") as u32;
        }
        Ok(limits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::{create_pool, run_migrations};

    async fn pool() -> SqlitePool {
        let p = create_pool("sqlite::memory:").await.unwrap();
        run_migrations(&p).await.unwrap();
        p
    }

    #[tokio::test]
    async fn load_default_then_accumulate() {
        let p = pool().await;
        let repo = FirewallStateRepository::new(&p);

        // No row yet → zeros.
        let s0 = repo.load("2026-06-03").await.unwrap();
        assert_eq!(s0.runs_in_window, 0);
        assert_eq!(s0.auto_applies_in_window, 0);
        assert_eq!(s0.consecutive_failures, 0);
        assert!(!s0.kill_switch, "kill switch is never persisted");

        // Persist some deltas, reload → they accumulated.
        let s1 = FirewallState {
            runs_in_window: 3,
            auto_applies_in_window: 2,
            consecutive_failures: 1,
            kill_switch: true, // must NOT be persisted
        };
        repo.save("2026-06-03", &s1).await.unwrap();
        let reloaded = repo.load("2026-06-03").await.unwrap();
        assert_eq!(reloaded.runs_in_window, 3);
        assert_eq!(reloaded.auto_applies_in_window, 2);
        assert_eq!(reloaded.consecutive_failures, 1);
        assert!(
            !reloaded.kill_switch,
            "kill switch must never round-trip through the DB"
        );

        // A different window is independent.
        assert_eq!(repo.load("2026-06-04").await.unwrap().runs_in_window, 0);
    }

    #[tokio::test]
    async fn limits_come_from_resident_budgets() {
        let p = pool().await;
        let repo = FirewallStateRepository::new(&p);
        // The 027 seed sets max_runs_per_day = 24 for every builtin mode.
        let lim = repo.limits_for("observer").await.unwrap();
        assert_eq!(lim.max_runs_per_window, 24);
        // An unknown mode → core defaults.
        let def = repo.limits_for("no_such_mode").await.unwrap();
        assert_eq!(def.max_runs_per_window, FirewallLimits::default().max_runs_per_window);
    }
}

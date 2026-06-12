//! DbSizeHistoryRepository (E2) — append-only DB-size snapshots written by the
//! weekly `db_optimize` brain job (migration 043).
//!
//! Each snapshot records the on-disk size (bytes), the bytes reclaimed by that
//! run's `incremental_vacuum`, and how many `brain_jobs` ran in the trailing
//! window (retention-job liveness). The doctor's DB-size-trend check reads the
//! two most recent rows to flag ANOMALOUS growth — never normal accumulation,
//! since raw turns are canonical and never deleted (doctrine).
//!
//! The tracker is itself bounded: [`record`] prunes the table to the most
//! recent [`MAX_SNAPSHOTS`] rows so the growth-watcher can never grow unbounded.

use chrono::Utc;
use sqlx::{Row, SqlitePool};

use crate::util::ts_to_text;

/// Keep at most this many snapshots — roughly two years of weekly runs.
pub const MAX_SNAPSHOTS: i64 = 104;

#[derive(Debug, Clone)]
pub struct DbSizeRow {
    pub id: String,
    pub size_bytes: i64,
    pub freed_bytes: i64,
    pub jobs_in_window: i64,
    pub ts: String,
}

pub struct DbSizeHistoryRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> DbSizeHistoryRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Append a snapshot, then prune to [`MAX_SNAPSHOTS`] most-recent rows.
    pub async fn record(
        &self,
        size_bytes: i64,
        freed_bytes: i64,
        jobs_in_window: i64,
    ) -> anyhow::Result<()> {
        let now = ts_to_text(&Utc::now());
        sqlx::query(
            "INSERT INTO db_size_history (id, size_bytes, freed_bytes, jobs_in_window, ts) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(size_bytes)
        .bind(freed_bytes)
        .bind(jobs_in_window)
        .bind(&now)
        .execute(self.pool)
        .await?;

        // Prune to the bounded window — keep the newest MAX_SNAPSHOTS rows.
        sqlx::query(
            "DELETE FROM db_size_history WHERE id NOT IN \
             (SELECT id FROM db_size_history ORDER BY ts DESC LIMIT ?)",
        )
        .bind(MAX_SNAPSHOTS)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// The most-recent `limit` snapshots, newest first.
    pub async fn recent(&self, limit: i64) -> anyhow::Result<Vec<DbSizeRow>> {
        let rows = sqlx::query(
            "SELECT id, size_bytes, freed_bytes, jobs_in_window, ts \
             FROM db_size_history ORDER BY ts DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| DbSizeRow {
                id: r.get("id"),
                size_bytes: r.get("size_bytes"),
                freed_bytes: r.get("freed_bytes"),
                jobs_in_window: r.get("jobs_in_window"),
                ts: r.get("ts"),
            })
            .collect())
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
    async fn record_and_recent_roundtrip() {
        let p = pool().await;
        let repo = DbSizeHistoryRepository::new(&p);
        assert!(repo.recent(5).await.unwrap().is_empty());

        repo.record(1000, 0, 3).await.unwrap();
        repo.record(1200, 40, 5).await.unwrap();

        let rows = repo.recent(5).await.unwrap();
        assert_eq!(rows.len(), 2);
        // Newest first.
        assert_eq!(rows[0].size_bytes, 1200);
        assert_eq!(rows[0].freed_bytes, 40);
        assert_eq!(rows[0].jobs_in_window, 5);
        assert_eq!(rows[1].size_bytes, 1000);
    }

    #[tokio::test]
    async fn prunes_to_bounded_window() {
        let p = pool().await;
        let repo = DbSizeHistoryRepository::new(&p);
        // Insert more than MAX_SNAPSHOTS rows — only the cap survives.
        for i in 0..(MAX_SNAPSHOTS + 10) {
            repo.record(1000 + i, 0, 1).await.unwrap();
        }
        let all = repo.recent(MAX_SNAPSHOTS + 100).await.unwrap();
        assert_eq!(all.len() as i64, MAX_SNAPSHOTS, "must prune to bounded window");
    }
}

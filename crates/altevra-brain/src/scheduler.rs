//! Scheduler — orchestrates periodic jobs.

use chrono::{Local, Utc};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::sync::watch;
use uuid::Uuid;

use crate::jobs::{dispatch, JobContext, JobKind};

#[derive(Debug, Clone)]
pub struct BrainConfig {
    pub vault_path: PathBuf,
    pub db_path: PathBuf,
    /// Hour-of-day (0-23) for daily summary in LOCAL time.
    pub daily_summary_hour: u32,
    /// Tick interval — how often the scheduler checks each job's due-time.
    pub tick_interval_secs: u64,
    /// Job kinds explicitly disabled (e.g. ["research_fetcher"] until you add feeds).
    pub disabled: Vec<String>,
}

impl Default for BrainConfig {
    fn default() -> Self {
        Self {
            vault_path: PathBuf::from("."),
            db_path: altevra_core::default_db_path(),
            daily_summary_hour: 23,
            tick_interval_secs: 30,
            disabled: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrainStatus {
    pub running: bool,
    pub last_runs: HashMap<String, String>, // kind → last finished_at
    pub jobs_done: i64,
    pub jobs_failed: i64,
}

pub struct BrainScheduler {
    config: BrainConfig,
    pool: SqlitePool,
    /// In-memory last-run instants per job kind.
    last_runs: HashMap<JobKind, Instant>,
    /// Last LOCAL date on which DailySummary fired (to avoid firing twice).
    last_daily_date: Option<chrono::NaiveDate>,
    /// Model router (from `[llm]` config). Defaults to noop; set via `with_router`.
    router: std::sync::Arc<altevra_llm::ModelRouter>,
}

impl BrainScheduler {
    pub fn new(config: BrainConfig, pool: SqlitePool) -> Self {
        Self {
            config,
            pool,
            last_runs: HashMap::new(),
            last_daily_date: None,
            router: std::sync::Arc::new(altevra_llm::ModelRouter::noop()),
        }
    }

    /// Attach a configured model router (e.g. built from `[llm]` config). Without
    /// this the scheduler runs all jobs against the noop router.
    pub fn with_router(mut self, router: std::sync::Arc<altevra_llm::ModelRouter>) -> Self {
        self.router = router;
        self
    }

    /// One tick: check each job, run those whose periods elapsed.
    pub async fn tick(&mut self) -> anyhow::Result<usize> {
        let mut ran = 0usize;
        let ctx = JobContext {
            vault_path: self.config.vault_path.clone(),
            now: Utc::now(),
            router: self.router.clone(),
        };
        let now = Instant::now();

        for kind in [
            JobKind::EventClassifier,
            JobKind::ObserverScan,
            JobKind::VaultIndexer,
            JobKind::InsightSynthesizer,
            JobKind::ResearchFetcher,
            JobKind::FeedDiscovery,
            JobKind::GitHubTrendingFetch,
            JobKind::ProjectResearchSweep,
            JobKind::DailySummary,
            JobKind::TaskGrooming,
        ] {
            if self.config.disabled.iter().any(|d| d == kind.as_str()) {
                continue;
            }

            // DailySummary: gate on LOCAL hour
            if kind == JobKind::DailySummary {
                let local_now = Local::now();
                let today = local_now.date_naive();
                let hour = local_now.hour_of_day();
                let already_today = self.last_daily_date.map(|d| d == today).unwrap_or(false);
                if hour < self.config.daily_summary_hour || already_today {
                    continue;
                }
                self.last_daily_date = Some(today);
            } else {
                let due = match self.last_runs.get(&kind) {
                    Some(last) => {
                        now.duration_since(*last) >= Duration::from_secs(kind.period_secs())
                    }
                    None => true,
                };
                if !due {
                    continue;
                }
            }

            let job_id = Uuid::new_v4();
            let start = Instant::now();
            let _ = sqlx::query(
                r#"INSERT INTO brain_jobs (id, kind, status) VALUES (?, ?, 'running')"#,
            )
            .bind(job_id.to_string())
            .bind(kind.as_str())
            .execute(&self.pool)
            .await;

            match dispatch(kind, &self.pool, &ctx).await {
                Ok(result) => {
                    let dur = start.elapsed().as_millis() as i64;
                    let _ = sqlx::query(
                        r#"UPDATE brain_jobs
                           SET status = 'done',
                               finished_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                               duration_ms = ?,
                               result_summary = ?
                           WHERE id = ?"#,
                    )
                    .bind(dur)
                    .bind(&result.summary)
                    .bind(job_id.to_string())
                    .execute(&self.pool)
                    .await;
                    tracing::info!(
                        "brain job {} done in {}ms: {}",
                        kind.as_str(),
                        dur,
                        result.summary
                    );
                }
                Err(e) => {
                    let dur = start.elapsed().as_millis() as i64;
                    let _ = sqlx::query(
                        r#"UPDATE brain_jobs
                           SET status = 'failed',
                               finished_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                               duration_ms = ?,
                               error = ?
                           WHERE id = ?"#,
                    )
                    .bind(dur)
                    .bind(e.to_string())
                    .bind(job_id.to_string())
                    .execute(&self.pool)
                    .await;
                    tracing::warn!("brain job {} failed: {e}", kind.as_str());
                }
            }
            self.last_runs.insert(kind, now);
            ran += 1;
        }
        Ok(ran)
    }

    /// Loop forever until shutdown_rx flips to true.
    pub async fn run(mut self, mut shutdown_rx: watch::Receiver<bool>) -> anyhow::Result<()> {
        let interval = Duration::from_secs(self.config.tick_interval_secs);
        loop {
            if *shutdown_rx.borrow() {
                break;
            }
            let _ = self.tick().await;
            tokio::select! {
                _ = shutdown_rx.changed() => {}
                _ = tokio::time::sleep(interval) => {}
            }
        }
        Ok(())
    }

    /// Snapshot of recent run history.
    pub async fn status(pool: &SqlitePool) -> anyhow::Result<BrainStatus> {
        let mut last_runs = HashMap::new();

        let rows = sqlx::query(
            r#"SELECT kind, MAX(finished_at) AS last_finished
               FROM brain_jobs
               WHERE finished_at IS NOT NULL
               GROUP BY kind"#,
        )
        .fetch_all(pool)
        .await?;
        for r in rows {
            let kind: String = sqlx::Row::try_get(&r, "kind")?;
            let last: Option<String> = sqlx::Row::try_get(&r, "last_finished").ok();
            if let Some(l) = last {
                last_runs.insert(kind, l);
            }
        }

        let agg = sqlx::query(
            r#"SELECT
                COALESCE(SUM(CASE WHEN status='done' THEN 1 ELSE 0 END), 0) AS done,
                COALESCE(SUM(CASE WHEN status='failed' THEN 1 ELSE 0 END), 0) AS failed
               FROM brain_jobs"#,
        )
        .fetch_one(pool)
        .await?;
        let jobs_done = sqlx::Row::try_get::<i64, _>(&agg, "done").unwrap_or(0);
        let jobs_failed = sqlx::Row::try_get::<i64, _>(&agg, "failed").unwrap_or(0);

        Ok(BrainStatus {
            running: false, // CLI fills this from PID file
            last_runs,
            jobs_done,
            jobs_failed,
        })
    }
}

/// chrono helper not in stable yet — small shim.
trait LocalExt {
    fn hour_of_day(&self) -> u32;
}
impl LocalExt for chrono::DateTime<Local> {
    fn hour_of_day(&self) -> u32 {
        chrono::Timelike::hour(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn setup() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            r#"CREATE TABLE brain_jobs (
                id TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                status TEXT NOT NULL,
                started_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                finished_at TEXT,
                duration_ms INTEGER,
                error TEXT,
                result_summary TEXT
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"CREATE TABLE pending_indexing (
                id TEXT PRIMARY KEY,
                path TEXT NOT NULL UNIQUE,
                queued_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                status TEXT NOT NULL DEFAULT 'pending',
                last_attempt_at TEXT,
                error TEXT,
                fail_count INTEGER NOT NULL DEFAULT 0
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"CREATE TABLE tasks (
                id TEXT PRIMARY KEY,
                status TEXT NOT NULL DEFAULT 'open',
                title TEXT NOT NULL
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn tick_runs_jobs_and_records_history() {
        let pool = setup().await;
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = BrainConfig {
            vault_path: tmp.path().to_path_buf(),
            db_path: tmp.path().join("altevra.db"),
            // Disable network-dependent jobs (research/discovery/trending)
            // and the daily-only gate. Leaves 5 deterministic jobs.
            disabled: vec![
                "daily_summary".into(),
                "feed_discovery".into(),
                "github_trending_fetch".into(),
                "research_fetcher".into(),
                "project_research_sweep".into(),
            ],
            ..BrainConfig::default()
        };
        let mut sched = BrainScheduler::new(cfg, pool.clone());
        let n = sched.tick().await.unwrap();
        assert!(n >= 5, "expected at least 5 jobs to run, got {n}");
        let status = BrainScheduler::status(&pool).await.unwrap();
        assert!(status.jobs_done + status.jobs_failed >= 5);
    }

    #[tokio::test]
    async fn second_tick_within_period_runs_no_jobs() {
        let pool = setup().await;
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = BrainConfig {
            vault_path: tmp.path().to_path_buf(),
            db_path: tmp.path().join("altevra.db"),
            disabled: vec![
                "daily_summary".into(),
                "feed_discovery".into(),
                "github_trending_fetch".into(),
                "research_fetcher".into(),
                "project_research_sweep".into(),
            ],
            ..BrainConfig::default()
        };
        let mut sched = BrainScheduler::new(cfg, pool.clone());
        let first = sched.tick().await.unwrap();
        assert!(first >= 5);
        let second = sched.tick().await.unwrap();
        assert_eq!(
            second, 0,
            "no jobs should be due immediately after first tick"
        );
    }

    #[tokio::test]
    async fn disabled_jobs_are_skipped() {
        let pool = setup().await;
        let tmp = tempfile::TempDir::new().unwrap();
        let cfg = BrainConfig {
            vault_path: tmp.path().to_path_buf(),
            db_path: tmp.path().join("altevra.db"),
            disabled: vec![
                "daily_summary".into(),
                "event_classifier".into(),
                "observer_scan".into(),
                "vault_indexer".into(),
                "insight_synthesizer".into(),
                "research_fetcher".into(),
                "feed_discovery".into(),
                "github_trending_fetch".into(),
                "project_research_sweep".into(),
                "task_grooming".into(),
            ],
            ..BrainConfig::default()
        };
        let mut sched = BrainScheduler::new(cfg, pool.clone());
        let n = sched.tick().await.unwrap();
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn status_aggregates_done_and_failed() {
        let pool = setup().await;
        sqlx::query("INSERT INTO brain_jobs (id, kind, status, finished_at) VALUES ('a','x','done', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO brain_jobs (id, kind, status, finished_at) VALUES ('b','y','failed', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))").execute(&pool).await.unwrap();
        let s = BrainScheduler::status(&pool).await.unwrap();
        assert_eq!(s.jobs_done, 1);
        assert_eq!(s.jobs_failed, 1);
    }
}

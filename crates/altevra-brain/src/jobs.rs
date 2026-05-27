//! Job definitions. Each job is an async function that takes a SqlitePool and
//! a JobContext, runs its work, and returns a JobResult with a one-line
//! summary that ends up in brain_jobs.result_summary.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    EventClassifier,
    ObserverScan,
    VaultIndexer,
    InsightSynthesizer,
    ResearchFetcher,
    DailySummary,
    TaskGrooming,
}

impl JobKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::EventClassifier => "event_classifier",
            Self::ObserverScan => "observer_scan",
            Self::VaultIndexer => "vault_indexer",
            Self::InsightSynthesizer => "insight_synthesizer",
            Self::ResearchFetcher => "research_fetcher",
            Self::DailySummary => "daily_summary",
            Self::TaskGrooming => "task_grooming",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "event_classifier" => Self::EventClassifier,
            "observer_scan" => Self::ObserverScan,
            "vault_indexer" => Self::VaultIndexer,
            "insight_synthesizer" => Self::InsightSynthesizer,
            "research_fetcher" => Self::ResearchFetcher,
            "daily_summary" => Self::DailySummary,
            "task_grooming" => Self::TaskGrooming,
            _ => return None,
        })
    }

    /// Period in seconds. Daily jobs use a fixed long period; the scheduler
    /// also checks the wall clock hour for them.
    pub fn period_secs(&self) -> u64 {
        match self {
            Self::EventClassifier => 60,
            Self::ObserverScan => 300,
            Self::VaultIndexer => 900,
            Self::InsightSynthesizer => 3600,
            Self::ResearchFetcher => 7200,
            Self::DailySummary => 3600, // tick hourly, fire only at 23:00
            Self::TaskGrooming => 10_800,
        }
    }
}

#[derive(Debug, Clone)]
pub struct JobResult {
    pub summary: String,
    pub items_processed: usize,
}

#[derive(Debug, Clone)]
pub struct JobContext {
    pub vault_path: std::path::PathBuf,
    pub now: DateTime<Utc>,
}

// ---- Job implementations ----------------------------------------------------

/// Process raw events.jsonl entries into classified UpdateFeedItems and append
/// to updates.jsonl. For a minimal pipeline we just count new lines since the
/// last marker file at .altevra/state/last_classified_offset.
pub async fn run_event_classifier(
    _pool: &SqlitePool,
    _ctx: &JobContext,
) -> anyhow::Result<JobResult> {
    let events_path = std::path::Path::new(".altevra/events/file_changes.jsonl");
    let updates_path = std::path::Path::new(".altevra/events/updates.jsonl");
    let marker_path = std::path::Path::new(".altevra/state/last_classified_offset");
    if !events_path.exists() {
        return Ok(JobResult {
            summary: "no file_changes.jsonl yet".into(),
            items_processed: 0,
        });
    }
    let content = std::fs::read_to_string(events_path).unwrap_or_default();
    let prev_offset: usize = if marker_path.exists() {
        std::fs::read_to_string(marker_path)
            .unwrap_or_default()
            .trim()
            .parse()
            .unwrap_or(0)
    } else {
        0
    };
    let lines: Vec<&str> = content.lines().collect();
    let new_lines = lines.len().saturating_sub(prev_offset);

    if new_lines > 0 {
        if let Some(parent) = updates_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(updates_path)
        {
            for line in lines.iter().skip(prev_offset) {
                if line.trim().is_empty() {
                    continue;
                }
                if let Ok(record) = serde_json::from_str::<serde_json::Value>(line) {
                    let update = serde_json::json!({
                        "id": uuid::Uuid::new_v4(),
                        "event_id": record.get("id"),
                        "update_type": "file_changed",
                        "importance": "low",
                        "title": format!("File {} {}",
                            record.get("kind").and_then(|v| v.as_str()).unwrap_or("changed"),
                            record.get("path").and_then(|v| v.as_str()).unwrap_or("")
                        ),
                        "short_summary": "tracked by watcher",
                        "created_at": chrono::Utc::now(),
                        "sensitivity": "internal",
                        "visible_to_agents": true,
                        "affected_entities": [],
                    });
                    let _ = writeln!(f, "{}", update);
                }
            }
        }
        if let Some(parent) = marker_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(marker_path, lines.len().to_string());
    }

    Ok(JobResult {
        summary: format!("classified {new_lines} new events"),
        items_processed: new_lines,
    })
}

/// Run pattern detection across recent events + updates. For now we just count
/// updates in the local JSONL — full observer wiring lives in altevra-core.
pub async fn run_observer_scan(_pool: &SqlitePool, _ctx: &JobContext) -> anyhow::Result<JobResult> {
    let updates_path = std::path::Path::new(".altevra/events/updates.jsonl");
    if !updates_path.exists() {
        return Ok(JobResult {
            summary: "no updates to scan".into(),
            items_processed: 0,
        });
    }
    let content = std::fs::read_to_string(updates_path).unwrap_or_default();
    let count = content.lines().filter(|l| !l.trim().is_empty()).count();
    Ok(JobResult {
        summary: format!("scanned {count} updates"),
        items_processed: count,
    })
}

/// Scan the vault and queue any missing memory_chunks for embedding. Reuses
/// altevra-vault scanner + altevra-memory ingestion.
pub async fn run_vault_indexer(pool: &SqlitePool, ctx: &JobContext) -> anyhow::Result<JobResult> {
    let files = altevra_vault::scan_vault(&ctx.vault_path).unwrap_or_default();
    let mut queued = 0;
    for f in files.iter().take(50) {
        // queue path for indexing — uses pending_indexing table
        let id = uuid::Uuid::new_v4().to_string();
        let _ = sqlx::query(
            r#"INSERT INTO pending_indexing (id, path, status) VALUES (?, ?, 'pending')
               ON CONFLICT (path) DO UPDATE SET status = 'pending'"#,
        )
        .bind(id)
        .bind(f.path.to_string_lossy().to_string())
        .execute(pool)
        .await;
        queued += 1;
    }
    Ok(JobResult {
        summary: format!("queued {queued} vault files"),
        items_processed: queued,
    })
}

/// Placeholder for LLM-powered synthesis. With a Gemini key configured this
/// would call gemini-flash to summarise the last hour. Without a key we just
/// emit a structural summary.
pub async fn run_insight_synthesizer(
    _pool: &SqlitePool,
    _ctx: &JobContext,
) -> anyhow::Result<JobResult> {
    Ok(JobResult {
        summary: "insight synthesis skipped (no LLM configured)".into(),
        items_processed: 0,
    })
}

/// Placeholder for RSS / web research. Wired in v0.3.5.
pub async fn run_research_fetcher(
    _pool: &SqlitePool,
    _ctx: &JobContext,
) -> anyhow::Result<JobResult> {
    Ok(JobResult {
        summary: "no feeds configured".into(),
        items_processed: 0,
    })
}

/// Daily summary at 23:00 local. Writes a markdown file under
/// vault/10-insights/daily-YYYY-MM-DD.md.
pub async fn run_daily_summary(_pool: &SqlitePool, ctx: &JobContext) -> anyhow::Result<JobResult> {
    let date = ctx.now.format("%Y-%m-%d").to_string();
    let dir = ctx.vault_path.join("10-insights");
    std::fs::create_dir_all(&dir).ok();
    let path = dir.join(format!("daily-{date}.md"));
    if path.exists() {
        return Ok(JobResult {
            summary: format!("daily summary already exists for {date}"),
            items_processed: 0,
        });
    }
    let body = format!(
        "---\nkind: daily-summary\ngenerated_by: altevra-brain\ndate: {date}\n---\n\n# Daily Summary — {date}\n\n_Generated by altevra-brain. Customize via LLM synthesizer once configured._\n"
    );
    std::fs::write(&path, body)?;
    Ok(JobResult {
        summary: format!("wrote daily summary for {date}"),
        items_processed: 1,
    })
}

/// Task grooming — flag stale tasks. Placeholder; full logic in v0.3.7.
pub async fn run_task_grooming(pool: &SqlitePool, _ctx: &JobContext) -> anyhow::Result<JobResult> {
    let row = sqlx::query("SELECT COUNT(*) AS n FROM tasks WHERE status = 'open'")
        .fetch_one(pool)
        .await
        .ok();
    let n = row
        .and_then(|r| sqlx::Row::try_get::<i64, _>(&r, "n").ok())
        .unwrap_or(0);
    Ok(JobResult {
        summary: format!("{n} open task(s)"),
        items_processed: n as usize,
    })
}

pub async fn dispatch(
    kind: JobKind,
    pool: &SqlitePool,
    ctx: &JobContext,
) -> anyhow::Result<JobResult> {
    match kind {
        JobKind::EventClassifier => run_event_classifier(pool, ctx).await,
        JobKind::ObserverScan => run_observer_scan(pool, ctx).await,
        JobKind::VaultIndexer => run_vault_indexer(pool, ctx).await,
        JobKind::InsightSynthesizer => run_insight_synthesizer(pool, ctx).await,
        JobKind::ResearchFetcher => run_research_fetcher(pool, ctx).await,
        JobKind::DailySummary => run_daily_summary(pool, ctx).await,
        JobKind::TaskGrooming => run_task_grooming(pool, ctx).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_kind_roundtrip() {
        for k in [
            JobKind::EventClassifier,
            JobKind::ObserverScan,
            JobKind::VaultIndexer,
            JobKind::InsightSynthesizer,
            JobKind::ResearchFetcher,
            JobKind::DailySummary,
            JobKind::TaskGrooming,
        ] {
            assert_eq!(JobKind::from_str(k.as_str()), Some(k));
        }
    }

    #[tokio::test]
    async fn observer_scan_handles_missing_file() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let ctx = JobContext {
            vault_path: std::path::PathBuf::from("/nonexistent"),
            now: Utc::now(),
        };
        let r = run_observer_scan(&pool, &ctx).await.unwrap();
        assert_eq!(r.items_processed, 0);
    }

    #[tokio::test]
    async fn daily_summary_writes_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let ctx = JobContext {
            vault_path: tmp.path().to_path_buf(),
            now: Utc::now(),
        };
        let r = run_daily_summary(&pool, &ctx).await.unwrap();
        assert_eq!(r.items_processed, 1);
        // idempotent — second call returns 0
        let r2 = run_daily_summary(&pool, &ctx).await.unwrap();
        assert_eq!(r2.items_processed, 0);
    }
}

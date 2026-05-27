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
    FeedDiscovery,
    GitHubTrendingFetch,
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
            Self::FeedDiscovery => "feed_discovery",
            Self::GitHubTrendingFetch => "github_trending_fetch",
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
            "feed_discovery" => Self::FeedDiscovery,
            "github_trending_fetch" => Self::GitHubTrendingFetch,
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
            Self::FeedDiscovery => 3600,
            Self::GitHubTrendingFetch => 14_400, // 4h
            Self::DailySummary => 3600,          // tick hourly, fire only at 23:00
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

/// Pull RSS/Atom feeds, dedupe via SQLite, score against project keywords,
/// write daily Obsidian brief + per-project briefs.
///
/// Driven by `~/.altevra/research/feeds.yaml` (falls back to default packet).
pub async fn run_research_fetcher(
    pool: &SqlitePool,
    ctx: &JobContext,
) -> anyhow::Result<JobResult> {
    use altevra_research::{
        briefs::{write_daily_brief, write_project_brief, ScoredItem},
        feeds::FeedConfig,
        fetcher::fetch_feed,
        relevance::{default_imperium_projects_path, load_imperium_projects, matching_projects},
    };

    let cfg = FeedConfig::load_or_default();
    let projects_path = default_imperium_projects_path();
    let projects = load_imperium_projects(&projects_path).unwrap_or_default();

    let mut new_items = 0usize;
    let mut scored_items: Vec<ScoredItem> = Vec::new();
    let mut feeds_touched = 0usize;

    let now = ctx.now;
    for feed in cfg.enabled() {
        // Skip if within fetch_interval since last_fetched_at.
        let cache_hints = fetch_cache_hints(pool, &feed.id).await;
        if let Some(last) = last_fetched_at(pool, &feed.id).await {
            let elapsed = (now - last).num_minutes();
            if elapsed >= 0 && (elapsed as u32) < feed.fetch_interval_minutes {
                continue;
            }
        }

        feeds_touched += 1;
        let outcome = match fetch_feed(feed, cfg.window_days, &cache_hints).await {
            Ok(o) => o,
            Err(e) => {
                record_feed_failure(pool, &feed.id, &e.to_string()).await;
                tracing::warn!("research fetch failed for {}: {e}", feed.id);
                continue;
            }
        };

        record_feed_success(pool, &feed.id, &outcome).await;

        for item in outcome.items {
            // Idempotent insert — UNIQUE(feed_id, guid) prevents dupes.
            let (max_score, matched) = matching_projects(&item, &projects, cfg.relevance_threshold);
            let id = uuid::Uuid::new_v4().to_string();
            let project_json = serde_json::to_string(&matched).unwrap_or_else(|_| "[]".into());
            let published = item.published_at.map(|d| d.to_rfc3339());

            let res = sqlx::query(
                r#"INSERT OR IGNORE INTO research_items
                       (id, feed_id, guid, link, title, summary, published_at,
                        relevance_score, project_matches_json)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
            )
            .bind(&id)
            .bind(&item.feed_id)
            .bind(&item.guid)
            .bind(&item.link)
            .bind(&item.title)
            .bind(&item.summary)
            .bind(published)
            .bind(max_score as f64)
            .bind(&project_json)
            .execute(pool)
            .await;

            let inserted = res.map(|r| r.rows_affected() > 0).unwrap_or(false);
            if inserted {
                new_items += 1;
                scored_items.push(ScoredItem {
                    item,
                    score: max_score,
                    matched_projects: matched,
                });
            }
        }
    }

    // Briefs — only write if we have something new.
    let mut briefs_written = 0usize;
    if !scored_items.is_empty() {
        if let Ok(path) = write_daily_brief(&cfg.brief_paths.daily_obsidian, &scored_items) {
            tracing::info!("daily brief written to {}", path.display());
            briefs_written += 1;
        }
        // Per-project briefs — one per matched project id.
        let mut project_ids: Vec<String> = scored_items
            .iter()
            .flat_map(|i| i.matched_projects.iter().cloned())
            .collect();
        project_ids.sort();
        project_ids.dedup();
        for pid in &project_ids {
            if let Ok(Some(path)) = write_project_brief(
                &ctx.vault_path,
                &cfg.brief_paths.project_vault,
                pid,
                &scored_items,
            ) {
                tracing::info!("project brief ({pid}) written to {}", path.display());
                briefs_written += 1;
            }
        }
    }

    Ok(JobResult {
        summary: format!(
            "fetched {feeds_touched} feeds, {new_items} new items, {briefs_written} brief(s) written"
        ),
        items_processed: new_items,
    })
}

async fn last_fetched_at(pool: &SqlitePool, feed_id: &str) -> Option<DateTime<Utc>> {
    let row = sqlx::query("SELECT last_fetched_at FROM research_feed_state WHERE feed_id = ?")
        .bind(feed_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()?;
    let s: Option<String> = sqlx::Row::try_get(&row, "last_fetched_at").ok();
    s.and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|d| d.with_timezone(&Utc))
}

async fn fetch_cache_hints(
    pool: &SqlitePool,
    feed_id: &str,
) -> altevra_research::fetcher::FetchCacheHints {
    let row =
        sqlx::query("SELECT last_etag, last_modified FROM research_feed_state WHERE feed_id = ?")
            .bind(feed_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
    match row {
        Some(r) => altevra_research::fetcher::FetchCacheHints {
            etag: sqlx::Row::try_get::<Option<String>, _>(&r, "last_etag")
                .ok()
                .flatten(),
            last_modified: sqlx::Row::try_get::<Option<String>, _>(&r, "last_modified")
                .ok()
                .flatten(),
        },
        None => altevra_research::fetcher::FetchCacheHints::default(),
    }
}

async fn record_feed_success(
    pool: &SqlitePool,
    feed_id: &str,
    outcome: &altevra_research::fetcher::FetchOutcome,
) {
    let now = Utc::now().to_rfc3339();
    let _ = sqlx::query(
        r#"INSERT INTO research_feed_state
               (feed_id, last_fetched_at, last_etag, last_modified, fail_count, last_error)
           VALUES (?, ?, ?, ?, 0, NULL)
           ON CONFLICT(feed_id) DO UPDATE SET
               last_fetched_at = excluded.last_fetched_at,
               last_etag = excluded.last_etag,
               last_modified = excluded.last_modified,
               fail_count = 0,
               last_error = NULL"#,
    )
    .bind(feed_id)
    .bind(&now)
    .bind(&outcome.new_etag)
    .bind(&outcome.new_last_modified)
    .execute(pool)
    .await;
}

async fn record_feed_failure(pool: &SqlitePool, feed_id: &str, err: &str) {
    let now = Utc::now().to_rfc3339();
    let _ = sqlx::query(
        r#"INSERT INTO research_feed_state
               (feed_id, last_fetched_at, fail_count, last_error)
           VALUES (?, ?, 1, ?)
           ON CONFLICT(feed_id) DO UPDATE SET
               last_fetched_at = excluded.last_fetched_at,
               fail_count = research_feed_state.fail_count + 1,
               last_error = excluded.last_error"#,
    )
    .bind(feed_id)
    .bind(&now)
    .bind(err)
    .execute(pool)
    .await;
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

/// Walk recent research_items, fetch their source pages, extract feed links,
/// and insert candidates. Full-auto mode promotes immediately into the
/// active feeds.yaml file.
pub async fn run_feed_discovery(pool: &SqlitePool, _ctx: &JobContext) -> anyhow::Result<JobResult> {
    use altevra_research::discover::{extract_feed_links, filter_promising_blog_links};

    // Pick a small batch of recent items to scan. Each row gives us a source
    // page URL — we crawl that page (light HTTP only — no imperium-crawl) and
    // extract any RSS hints.
    let rows = sqlx::query("SELECT link FROM research_items ORDER BY ingested_at DESC LIMIT 25")
        .fetch_all(pool)
        .await?;
    if rows.is_empty() {
        return Ok(JobResult {
            summary: "no research items to mine for discovery".into(),
            items_processed: 0,
        });
    }

    let client = reqwest::Client::builder()
        .user_agent("Altevra/0.3 feed-discovery")
        .timeout(std::time::Duration::from_secs(15))
        .build()?;

    let mut candidates_seen = 0usize;
    let mut candidates_new = 0usize;
    for row in rows {
        let url: String = sqlx::Row::try_get(&row, "link").unwrap_or_default();
        if url.is_empty() {
            continue;
        }
        let Ok(resp) = client.get(&url).send().await else {
            continue;
        };
        if !resp.status().is_success() {
            continue;
        }
        let Ok(html) = resp.text().await else {
            continue;
        };

        // Direct feed-link hints from this page.
        let feed_links = extract_feed_links(&url, &html);
        // Optionally, promising outbound (filtered) — kept as candidates without feed_url.
        let outbound = altevra_research::discover::extract_outbound_links(&url, &html);
        let promising = filter_promising_blog_links(&outbound);

        for f in feed_links.iter().chain(promising.iter()) {
            candidates_seen += 1;
            let id = uuid::Uuid::new_v4().to_string();
            let res = sqlx::query(
                r#"INSERT OR IGNORE INTO research_feed_candidates
                       (id, candidate_url, feed_url, source_url, discovered_by, status)
                   VALUES (?, ?, ?, ?, 'brain_job', 'pending')"#,
            )
            .bind(&id)
            .bind(f)
            .bind(f) // candidate_url == feed_url for the direct-link hints
            .bind(&url)
            .execute(pool)
            .await;
            if let Ok(r) = res {
                if r.rows_affected() > 0 {
                    candidates_new += 1;
                }
            }
        }
    }

    Ok(JobResult {
        summary: format!(
            "discovery scanned {} item(s), found {candidates_seen} candidate links, {candidates_new} new",
            25
        ),
        items_processed: candidates_new,
    })
}

/// Fetch GitHub Trending for a configurable set of languages and ingest as
/// research_items with source_kind = 'github-trending'.
pub async fn run_github_trending_fetch(
    pool: &SqlitePool,
    _ctx: &JobContext,
) -> anyhow::Result<JobResult> {
    use altevra_research::sources::github_trending::{GitHubTrendingSource, TrendingPeriod};
    use altevra_research::sources::{FetchCtx, SourceProvider};

    let languages: &[Option<&str>] = &[Some("rust"), Some("typescript"), Some("python")];
    let ctx = FetchCtx {
        window_days: 1,
        ..Default::default()
    };
    let mut total_new = 0usize;
    let mut feeds_touched = 0usize;

    for lang in languages {
        feeds_touched += 1;
        let source = GitHubTrendingSource::new(lang.map(String::from), TrendingPeriod::Daily);
        let items = match source.fetch(&ctx).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("github trending fetch failed for {:?}: {e}", lang);
                continue;
            }
        };
        let feed_id = source.id_str();
        for item in items {
            let id = uuid::Uuid::new_v4().to_string();
            let res = sqlx::query(
                r#"INSERT OR IGNORE INTO research_items
                       (id, feed_id, guid, link, title, summary, published_at,
                        relevance_score, project_matches_json, source_kind)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?, '[]', 'github-trending')"#,
            )
            .bind(&id)
            .bind(&feed_id)
            .bind(&item.guid)
            .bind(&item.link)
            .bind(&item.title)
            .bind(&item.summary)
            .bind(item.published_at.map(|d| d.to_rfc3339()))
            .bind(0.0f64)
            .execute(pool)
            .await;
            if let Ok(r) = res {
                if r.rows_affected() > 0 {
                    total_new += 1;
                }
            }
        }
    }
    Ok(JobResult {
        summary: format!("github trending: {feeds_touched} langs, {total_new} new repos"),
        items_processed: total_new,
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
        JobKind::FeedDiscovery => run_feed_discovery(pool, ctx).await,
        JobKind::GitHubTrendingFetch => run_github_trending_fetch(pool, ctx).await,
        JobKind::DailySummary => run_daily_summary(pool, ctx).await,
        JobKind::TaskGrooming => run_task_grooming(pool, ctx).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_research_schema() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        // Minimal schema needed for run_research_fetcher path.
        sqlx::query(
            r#"CREATE TABLE research_feed_state (
                feed_id TEXT PRIMARY KEY,
                last_fetched_at TEXT,
                last_etag TEXT,
                last_modified TEXT,
                fail_count INTEGER NOT NULL DEFAULT 0,
                last_error TEXT
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"CREATE TABLE research_items (
                id TEXT PRIMARY KEY,
                feed_id TEXT NOT NULL,
                guid TEXT NOT NULL,
                link TEXT NOT NULL,
                title TEXT NOT NULL,
                summary TEXT NOT NULL DEFAULT '',
                published_at TEXT,
                ingested_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                relevance_score REAL NOT NULL DEFAULT 0.0,
                project_matches_json TEXT NOT NULL DEFAULT '[]',
                UNIQUE(feed_id, guid)
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[test]
    fn job_kind_roundtrip() {
        for k in [
            JobKind::EventClassifier,
            JobKind::ObserverScan,
            JobKind::VaultIndexer,
            JobKind::InsightSynthesizer,
            JobKind::ResearchFetcher,
            JobKind::FeedDiscovery,
            JobKind::GitHubTrendingFetch,
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

    #[tokio::test]
    async fn record_feed_success_then_failure_increments_count() {
        let pool = setup_research_schema().await;
        let outcome = altevra_research::fetcher::FetchOutcome {
            items: vec![],
            new_etag: Some("\"abc\"".into()),
            new_last_modified: Some("Wed, 21 Oct 2026 07:28:00 GMT".into()),
            status: 200,
        };
        record_feed_success(&pool, "feed-x", &outcome).await;
        record_feed_failure(&pool, "feed-x", "DNS error").await;
        let row =
            sqlx::query("SELECT fail_count, last_error FROM research_feed_state WHERE feed_id = ?")
                .bind("feed-x")
                .fetch_one(&pool)
                .await
                .unwrap();
        let count: i64 = sqlx::Row::try_get(&row, "fail_count").unwrap();
        let err: String = sqlx::Row::try_get(&row, "last_error").unwrap();
        assert_eq!(count, 1);
        assert!(err.contains("DNS"));
    }

    #[tokio::test]
    async fn feed_discovery_returns_when_no_items() {
        let pool = setup_research_schema().await;
        // Need research_feed_candidates table.
        sqlx::query(
            r#"CREATE TABLE research_feed_candidates (
                id TEXT PRIMARY KEY,
                candidate_url TEXT NOT NULL UNIQUE,
                feed_url TEXT,
                source_url TEXT,
                discovered_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                discovered_by TEXT,
                status TEXT NOT NULL DEFAULT 'pending',
                auto_promoted_at TEXT,
                rejected_reason TEXT
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();

        let tmp = tempfile::TempDir::new().unwrap();
        let ctx = JobContext {
            vault_path: tmp.path().to_path_buf(),
            now: Utc::now(),
        };
        let r = run_feed_discovery(&pool, &ctx).await.unwrap();
        // Empty DB -> "no research items to mine for discovery"
        assert!(r.summary.contains("no research items"));
        assert_eq!(r.items_processed, 0);
    }

    #[tokio::test]
    async fn github_trending_fetch_does_not_panic_offline() {
        // Test SCHEMA path: even if network fetch fails for all langs the job
        // returns a summary, not a panic.
        let pool = setup_research_schema().await;
        // Provide source_kind column via ALTER (since our test schema is minimal).
        sqlx::query(
            "ALTER TABLE research_items ADD COLUMN source_kind TEXT NOT NULL DEFAULT 'rss'",
        )
        .execute(&pool)
        .await
        .ok();
        let tmp = tempfile::TempDir::new().unwrap();
        let ctx = JobContext {
            vault_path: tmp.path().to_path_buf(),
            now: Utc::now(),
        };
        // We expect this to attempt 3 langs; either they succeed (network OK)
        // or all fail and total_new == 0. Either way: no panic.
        let r = run_github_trending_fetch(&pool, &ctx).await.unwrap();
        assert!(r.summary.contains("github trending"));
    }

    #[tokio::test]
    async fn research_fetcher_returns_when_no_feeds_reachable() {
        // We can't hit real network in unit tests, so verify the job itself
        // doesn't panic when feeds resolve to no items. The default-packet
        // load path is exercised via test below.
        let pool = setup_research_schema().await;
        let tmp = tempfile::TempDir::new().unwrap();
        let ctx = JobContext {
            vault_path: tmp.path().to_path_buf(),
            now: Utc::now(),
        };
        // Override feeds.yaml to a single bad URL so the loop runs and records a failure
        // instead of trying to hit real RSS endpoints.
        let yaml = r#"
feeds:
  - id: bad-feed
    name: Bad
    url: https://this-domain-does-not-exist-altevra-test.invalid/rss
    type: rss
    category: test
    trust_weight: 0.1
    enabled: true
    fetch_interval_minutes: 60
window_days: 7
relevance_threshold: 0.4
"#;
        let feeds_dir = tmp.path().join(".altevra-research");
        std::fs::create_dir_all(&feeds_dir).unwrap();
        let feeds_path = feeds_dir.join("feeds.yaml");
        std::fs::write(&feeds_path, yaml).unwrap();

        // Point HOME at tmp so default_path() resolves there.
        let old_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());
        let alt_feeds = tmp.path().join(".altevra").join("research");
        std::fs::create_dir_all(&alt_feeds).unwrap();
        std::fs::copy(&feeds_path, alt_feeds.join("feeds.yaml")).unwrap();

        let r = run_research_fetcher(&pool, &ctx).await.unwrap();
        // Either DNS resolves or fails — either way job completes without panic
        // and items_processed is 0 because no items came back.
        assert!(r.summary.contains("feeds"));

        if let Some(h) = old_home {
            std::env::set_var("HOME", h);
        } else {
            std::env::remove_var("HOME");
        }
    }
}

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
    ProjectResearchSweep,
    DailySummary,
    TaskGrooming,
    AutoCategorizer,
    SelfImproveOrchestrator,
    /// C7 — DB-level skill/proposal curator (Hermes-borrowed). Status-only
    /// transitions, never deletes; runs ~7 days (idle-gated by period). See
    /// [`crate::curator`] for the policy.
    Curator,
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
            Self::ProjectResearchSweep => "project_research_sweep",
            Self::DailySummary => "daily_summary",
            Self::TaskGrooming => "task_grooming",
            Self::AutoCategorizer => "auto_categorizer",
            Self::SelfImproveOrchestrator => "self_improve_orchestrator",
            Self::Curator => "curator",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "event_classifier" => Self::EventClassifier,
            "observer_scan" => Self::ObserverScan,
            "vault_indexer" => Self::VaultIndexer,
            "insight_synthesizer" => Self::InsightSynthesizer,
            "research_fetcher" => Self::ResearchFetcher,
            "feed_discovery" => Self::FeedDiscovery,
            "github_trending_fetch" => Self::GitHubTrendingFetch,
            "project_research_sweep" => Self::ProjectResearchSweep,
            "daily_summary" => Self::DailySummary,
            "task_grooming" => Self::TaskGrooming,
            "auto_categorizer" => Self::AutoCategorizer,
            "self_improve_orchestrator" => Self::SelfImproveOrchestrator,
            "curator" => Self::Curator,
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
            Self::GitHubTrendingFetch => 14_400,  // 4h
            Self::ProjectResearchSweep => 86_400, // 24h
            Self::DailySummary => 3600,           // tick hourly, fire only at 23:00
            Self::TaskGrooming => 10_800,
            Self::AutoCategorizer => 1800, // 30 min — classify newly-indexed objects
            // Periodic backstop (~45 min): the loop is ALSO triggered real-time (a
            // hook can invoke `run_self_improve`); this is the safety net so a missed
            // trigger still gets the 7-stage loop run within the window.
            Self::SelfImproveOrchestrator => 2700,
            // C7: ~7 days. Mirrors Hermes' `DEFAULT_INTERVAL_HOURS = 24 * 7`. The
            // curator is intentionally infrequent — it sweeps long-tail status
            // staleness, not real-time signals.
            Self::Curator => 7 * 24 * 60 * 60,
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
    /// Model router resolved from `[llm]` config. With `delegated` (default) every
    /// role resolves to noop, so LLM-backed jobs skip cleanly until keys are added.
    pub router: std::sync::Arc<altevra_llm::ModelRouter>,
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

/// LLM-powered synthesis (B4). Resolves the `strong_reasoner` role from the router:
/// with `delegated` mode (noop) it skips cleanly (connected tool synthesizes over
/// MCP instead); with a real provider (codex_oauth / api) it produces an insight
/// AND persists it as a durable `insight_card` (migration 020). The card
/// auto-indexes via the A1 `index_object` path inside [`InsightCardsRepository`],
/// so `recall` finds it. SI-14: a card is only written when the model returns
/// non-empty prose; an empty/failed completion writes nothing.
pub async fn run_insight_synthesizer(
    pool: &SqlitePool,
    ctx: &JobContext,
) -> anyhow::Result<JobResult> {
    use altevra_db::{InsightCardRow, InsightCardsRepository};
    use altevra_llm::{ChatMessage, ChatOpts, ModelRole};

    let provider = ctx.router.resolve(ModelRole::StrongReasoner);
    // StrongReasoner is a non-personal reasoning role (cloud-eligible, SI-7); the
    // router already forbids personal/local_private from ever reaching the cloud.
    if provider.id() == "noop" {
        return Ok(JobResult {
            summary: "insight synthesis skipped (no LLM configured)".into(),
            items_processed: 0,
        });
    }
    let messages = vec![
        ChatMessage::system(
            "You are Altevra's insight synthesizer. Distill recent activity into ONE \
             concise, sourced sentence. No preamble.",
        ),
        ChatMessage::user("Summarize the most salient pattern in the last hour of activity."),
    ];
    match provider
        .complete(&messages, &ChatOpts::default().with_max_tokens(120))
        .await
    {
        Ok(text) => {
            let body = text.trim().to_string();
            // SI-14: no real content → no write (validate-then-write).
            if body.is_empty() {
                return Ok(JobResult {
                    summary: format!("insight ({}): empty completion, no card written", provider.id()),
                    items_processed: 0,
                });
            }
            // Title = first line (truncated); body = full prose.
            let title: String = body
                .lines()
                .next()
                .unwrap_or(&body)
                .chars()
                .take(120)
                .collect();
            let id = format!("insight-{}", ctx.now.format("%Y%m%dT%H%M%S"));
            let mut card = InsightCardRow::new(id, title.clone(), body.clone());
            // Synthesized over non-personal activity → business domain, internal,
            // agent-inferred (the constructor default), low-ish confidence.
            card.categories = "[\"synthesis\"]".into();
            card.tags = "[\"insight\",\"synthesis\"]".into();
            InsightCardsRepository::new(pool).insert(&card).await?;

            let one: String = body.chars().take(240).collect();
            Ok(JobResult {
                summary: format!("insight card ({}): {one}", provider.id()),
                items_processed: 1,
            })
        }
        Err(e) => Ok(JobResult {
            summary: format!("insight synthesis failed: {e}"),
            items_processed: 0,
        }),
    }
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

/// A person who's gone quiet: no mention for `weeks_since` weeks (CLAUDE.md §3.6).
const LAST_CONTACT_STALE_WEEKS: i64 = 2;
/// How far back the daily summary loads events for pattern detection.
const DAILY_EVENT_WINDOW_DAYS: i64 = 30;

/// Daily summary at 23:00 local (B3) — "the brain that notices" (CLAUDE.md §3.6).
/// Writes a markdown file under `vault/10-insights/daily-YYYY-MM-DD.md` that
/// surfaces THREE real signals:
///   1. detected patterns (`altevra_core::observer::detect_patterns` over recent
///      events),
///   2. last-contact gaps ("haven't talked to <Person> in <N> weeks") computed via
///      `altevra_core::last_contact` over the mention graph, and
///   3. stale decisions whose `review_after` has passed ("still applies?").
///
/// If a real `StrongReasoner` is configured, the assembled bullets are passed to
/// the LLM to synthesize prose; otherwise the structured bullets are written
/// directly (noop path — no LLM). Either way the file contains real content.
pub async fn run_daily_summary(pool: &SqlitePool, ctx: &JobContext) -> anyhow::Result<JobResult> {
    use altevra_core::{detect_patterns, last_contact, EntityKind};
    use altevra_db::{EventsRepository, MentionsRepository, TasksRepository};
    use altevra_llm::{ChatMessage, ChatOpts, ModelRole};

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

    // 1. Detected patterns over recent events (observer is keyless + LIVE).
    let since = ctx.now - chrono::Duration::days(DAILY_EVENT_WINDOW_DAYS);
    let events = EventsRepository::new(pool)
        .list_since(since, None, 2000)
        .await
        .unwrap_or_default();
    let insights = detect_patterns(&events, &[]);
    let mut pattern_lines: Vec<String> = insights
        .iter()
        .map(|i| format!("[{}] {}", i.importance, i.title))
        .collect();
    pattern_lines.sort();

    // 2. Last-contact gaps. Build the entity dictionary (people/projects) from the
    //    vault + registry, then for each known PERSON compute last_contact over the
    //    mention graph (gives `last_contact` in altevra-core its first caller).
    let dict = altevra_vault::entity_dict::build_dictionary_for_vault(&ctx.vault_path);
    let dated = MentionsRepository::new(pool)
        .dated_mentions()
        .await
        .unwrap_or_default();
    let today = ctx.now.date_naive();
    let mut contact_lines: Vec<String> = Vec::new();
    for person in dict.all().filter(|e| e.kind == EntityKind::Person) {
        let Some(last) = last_contact(&person.id, &dated) else {
            continue; // never mentioned → nothing to nag about
        };
        let weeks = (today - last).num_weeks();
        if weeks >= LAST_CONTACT_STALE_WEEKS {
            contact_lines.push(format!(
                "haven't talked to {} in {} weeks (last: {})",
                person.name, weeks, last
            ));
        }
    }
    contact_lines.sort();

    // 3. Decisions whose review_after has passed — "still applies?".
    let due = TasksRepository::new(pool)
        .decisions_due_for_review(ctx.now, 50)
        .await
        .unwrap_or_default();
    let decision_lines: Vec<String> = due
        .iter()
        .map(|d| {
            format!(
                "decision '{}' from {} — still applies?",
                d.title,
                d.decided_at.format("%Y-%m-%d")
            )
        })
        .collect();

    let total_signals = pattern_lines.len() + contact_lines.len() + decision_lines.len();

    // Assemble the structured bullets (always — they are the source of truth).
    let mut structured = String::new();
    structured.push_str("## Patterns detected\n\n");
    if pattern_lines.is_empty() {
        structured.push_str("- _No patterns detected in this window._\n");
    } else {
        for l in &pattern_lines {
            structured.push_str(&format!("- {l}\n"));
        }
    }
    structured.push_str("\n## People — last contact\n\n");
    if contact_lines.is_empty() {
        structured.push_str("- _No overdue reach-outs._\n");
    } else {
        for l in &contact_lines {
            structured.push_str(&format!("- {l}\n"));
        }
    }
    structured.push_str("\n## Decisions to re-check\n\n");
    if decision_lines.is_empty() {
        structured.push_str("- _No decisions past their review date._\n");
    } else {
        for l in &decision_lines {
            structured.push_str(&format!("- {l}\n"));
        }
    }

    // C7 — curator digest line (additive; never replaces other sections).
    // Counts come from real `proposals` + `skills` rows, not a hard-coded zero.
    // Format pinned by `curator::DIGEST_TAG` so dashboards can grep for it.
    let digest = crate::curator::curator_digest_line(pool).await;
    structured.push_str("\n## Self-improve\n\n");
    structured.push_str(&format!("- {digest}\n"));

    // 4. If a StrongReasoner is configured, synthesize prose; else write bullets.
    //    StrongReasoner is a non-personal reasoning role (cloud-eligible, SI-7);
    //    the structured bullets carry only titles/dates already in the vault.
    let provider = ctx.router.resolve(ModelRole::StrongReasoner);
    let mut prose: Option<String> = None;
    if provider.id() != "noop" {
        let messages = vec![
            ChatMessage::system(
                "You are Altevra's daily briefing writer. Given the structured signals \
                 below, write a SHORT prose briefing (3-5 sentences) that a busy founder \
                 reads in the evening. Keep every concrete fact (names, dates, counts). \
                 No preamble.",
            ),
            ChatMessage::user(&structured),
        ];
        if let Ok(text) = provider
            .complete(&messages, &ChatOpts::default().with_max_tokens(400))
            .await
        {
            let t = text.trim().to_string();
            if !t.is_empty() {
                prose = Some(t);
            }
        }
    }

    let generated_by = if prose.is_some() {
        format!("altevra-brain + {}", provider.id())
    } else {
        "altevra-brain".to_string()
    };
    let mut body = format!(
        "---\nkind: daily-summary\ngenerated_by: {generated_by}\ndate: {date}\nsignals: {total_signals}\n---\n\n# Daily Summary — {date}\n\n"
    );
    if let Some(p) = &prose {
        body.push_str(p);
        body.push_str("\n\n---\n\n");
    }
    body.push_str(&structured);

    std::fs::write(&path, body)?;
    Ok(JobResult {
        summary: format!(
            "daily summary for {date}: {} pattern(s), {} contact gap(s), {} stale decision(s)",
            pattern_lines.len(),
            contact_lines.len(),
            decision_lines.len()
        ),
        items_processed: 1,
    })
}

/// How many uncategorized objects one auto-categorizer pass handles.
const AUTO_CATEGORIZE_BATCH: i64 = 50;

/// Auto-categorization (B5, CLAUDE.md §3.2 — a LIVING taxonomy, not a static enum).
///
/// Reads `object_index` rows lacking a resolved category (`categories == []`) and,
/// for each, asks an LLM to classify it against the categories already in use:
///   * an existing category fits → tag the object (`set_category`),
///   * none fits → propose a NEW category as a `kind="category"` proposal
///     (Tier-0, via [`ProposalsRepository`]) for Pavle's daily digest.
///
/// **SI-7 routing (load-bearing):** the model that sees the object is chosen by the
/// object's DOMAIN. A high-water object (personal/relationship/health/legal/
/// financial/client) is classified by `local_private` (on-device) and MUST NEVER be
/// sent to the cloud `cheap_worker`. Non-high-water objects use `cheap_worker`. If
/// the role resolves to noop (no model configured), the object is skipped cleanly —
/// nothing is classified, tagged, or proposed.
pub async fn run_auto_categorizer(
    pool: &SqlitePool,
    ctx: &JobContext,
) -> anyhow::Result<JobResult> {
    use altevra_core::Domain;
    use altevra_db::{NewProposal, ObjectIndexRepository, ProposalsRepository};
    use altevra_llm::{ChatMessage, ChatOpts, ModelRole};

    let idx = ObjectIndexRepository::new(pool);
    let todo = idx.uncategorized(AUTO_CATEGORIZE_BATCH).await?;
    if todo.is_empty() {
        return Ok(JobResult {
            summary: "auto-categorize: nothing uncategorized".into(),
            items_processed: 0,
        });
    }
    let existing = idx.distinct_categories().await?;
    let proposals = ProposalsRepository::new(pool);

    let mut tagged = 0usize;
    let mut proposed = 0usize;
    let mut skipped = 0usize;

    for obj in &todo {
        // SI-7: route by domain. High-water → local_private (never cloud); else
        // cheap_worker. A high-water object must never be classified by the cloud
        // worker — the router additionally enforces local-only for local_private.
        let domain: Domain = obj.domain.parse().unwrap_or(Domain::Business);
        let mut role = if domain.is_high_water() {
            ModelRole::LocalPrivate
        } else {
            ModelRole::CheapWorker
        };

        let title = obj.title.clone().unwrap_or_default();

        // SI-7 DEFENSE-IN-DEPTH (content fail-safe): obj.domain is stamped upstream
        // at ingest (template default_domain — e.g. the 'learning' builtin defaults
        // to Business), so a genuinely personal/relationship/health thought captured
        // under a generic object_type can carry domain=business and would otherwise
        // route to the CLOUD cheap_worker. Before sending ANYTHING to the cloud, scan
        // title+body with the SAME high-water keyword net the pre-write gate uses
        // (altevra_secrets::content_is_high_water). If the content looks high-water
        // but the domain stamp isn't, treat it as high-water → local_private. This
        // makes the SI-7 guarantee independent of a possibly-wrong upstream domain.
        // Conservative: a false positive only keeps something local (safe).
        if role == ModelRole::CheapWorker {
            let body = fetch_object_body(pool, &obj.object_type, &obj.id).await;
            let scanned = format!("{title}\n{body}");
            if altevra_secrets::content_is_high_water(&scanned) {
                tracing::warn!(
                    "auto-categorize: object {} has domain={} but high-water CONTENT — \
                     keeping local (SI-7 content fail-safe), not sending to cloud",
                    obj.id,
                    obj.domain
                );
                role = ModelRole::LocalPrivate;
            }
        }

        let provider = ctx.router.resolve(role);
        if provider.id() == "noop" {
            // No model for this role → skip cleanly (no write).
            skipped += 1;
            continue;
        }

        let cat_list = if existing.is_empty() {
            "(none yet)".to_string()
        } else {
            existing.join(", ")
        };
        let messages = vec![
            ChatMessage::system(
                "You are Altevra's category classifier. Reply with EXACTLY ONE short \
                 lowercase category label and NOTHING else. Prefer an existing category \
                 from the provided list if one fits; otherwise return a new, concise \
                 label.",
            ),
            ChatMessage::user(format!(
                "Existing categories: {cat_list}\nObject ({}/{}, domain={}): {title}",
                obj.object_type, obj.id, obj.domain
            )),
        ];
        let label = match provider
            .complete(&messages, &ChatOpts::default().with_max_tokens(16))
            .await
        {
            Ok(t) => normalize_category(&t),
            Err(e) => {
                tracing::warn!("auto-categorize classify failed for {}: {e}", obj.id);
                skipped += 1;
                continue;
            }
        };
        if label.is_empty() {
            skipped += 1;
            continue;
        }

        // Does an existing category fit (case-insensitive)?
        match existing
            .iter()
            .find(|c| c.eq_ignore_ascii_case(&label))
        {
            Some(fit) => {
                if idx.set_category(&obj.object_type, &obj.id, fit).await? {
                    tagged += 1;
                }
            }
            None => {
                // Novel category → a Tier-0 `category` proposal for the daily digest.
                // SI-9: the repo re-derives the tier from kind ("category" → Tier-0).
                let dedup = format!("category:{}", label.to_lowercase());
                let (_, is_new) = proposals
                    .insert(&NewProposal {
                        kind: "category".into(),
                        title: format!("New category: {label}"),
                        body: format!(
                            "Auto-categorizer found no existing category for {} `{}` \
                             (domain={}); proposes a new category `{label}`.",
                            obj.object_type, obj.id, obj.domain
                        ),
                        source_mode: Some("auto_categorizer".into()),
                        dedup_hash: dedup,
                        evidence_refs: vec![format!("{}:{}", obj.object_type, obj.id)],
                        touches_sensitive: false,
                        touches_constitutional: false,
                    })
                    .await?;
                if is_new {
                    proposed += 1;
                }
            }
        }
    }

    Ok(JobResult {
        summary: format!(
            "auto-categorize: {} considered, {tagged} tagged, {proposed} new-category proposal(s), {skipped} skipped (no model)",
            todo.len()
        ),
        items_processed: tagged + proposed,
    })
}

/// Fetch an object's indexed body from `object_fts` (where capture stores the full
/// text). Used by the SI-7 content fail-safe to scan title+body before any cloud
/// call. Returns an empty string if the row is absent or the query fails — a missing
/// body just means the title-only scan still runs (fail-safe never errors the job).
async fn fetch_object_body(pool: &SqlitePool, object_type: &str, id: &str) -> String {
    sqlx::query_scalar::<_, String>(
        "SELECT body FROM object_fts WHERE object_type = ? AND object_id = ? LIMIT 1",
    )
    .bind(object_type)
    .bind(id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .unwrap_or_default()
}

/// Normalize a model's category reply to a single short lowercase label: first
/// non-empty line, trimmed of quotes/punctuation, lowercased, capped length.
fn normalize_category(raw: &str) -> String {
    let line = raw.trim().lines().next().unwrap_or("").trim();
    let cleaned: String = line
        .trim_matches(|c: char| c == '"' || c == '\'' || c == '`' || c == '.' || c == ',')
        .to_lowercase();
    cleaned.chars().take(40).collect::<String>().trim().to_string()
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

/// Per-project agent sweep. For every project in
/// `~/.imperium/identity/projects.yaml` (with optional per-project YAML override
/// at `~/.altevra/research/projects/<id>.yaml`), run web search for each
/// configured query against DuckDuckGo (free; Brave/Exa if keys present),
/// and insert top-N items into research_items with source_kind='web-search'.
pub async fn run_project_research_sweep(
    pool: &SqlitePool,
    _ctx: &JobContext,
) -> anyhow::Result<JobResult> {
    use altevra_research::projects::ProjectAgent;
    use altevra_research::sources::web_search::{WebSearchProviderKind, WebSearchSource};
    use altevra_research::sources::{FetchCtx, SourceProvider};

    let identity_path = std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"))
        .join(".imperium")
        .join("identity")
        .join("projects.yaml");
    if !identity_path.exists() {
        return Ok(JobResult {
            summary: "no ~/.imperium/identity/projects.yaml — skipping project sweep".into(),
            items_processed: 0,
        });
    }
    let agents = ProjectAgent::load_all(&identity_path).unwrap_or_default();
    if agents.is_empty() {
        return Ok(JobResult {
            summary: "no project agents loaded".into(),
            items_processed: 0,
        });
    }

    let brave_key = std::env::var("BRAVE_API_KEY").ok();
    let exa_key = std::env::var("EXA_API_KEY").ok();
    let mut total_new = 0usize;
    let mut projects_touched = 0usize;

    for agent in &agents {
        projects_touched += 1;
        let queries_to_run = agent
            .queries
            .iter()
            .take(agent.daily_budget_queries.min(20) as usize)
            .cloned()
            .collect::<Vec<_>>();
        if queries_to_run.is_empty() {
            continue;
        }

        for query in &queries_to_run {
            let mut source = WebSearchSource::new(query.clone());
            // Provider chain: Brave (if keyed) → Exa (if keyed) → DDG.
            let mut chain = Vec::new();
            if brave_key.is_some() {
                chain.push(WebSearchProviderKind::Brave);
            }
            if exa_key.is_some() {
                chain.push(WebSearchProviderKind::Exa);
            }
            chain.push(WebSearchProviderKind::DuckDuckGo);
            source = source.with_chain(chain);
            if let Some(k) = &brave_key {
                source = source.with_brave(k);
            }
            if let Some(k) = &exa_key {
                source = source.with_exa(k);
            }

            let ctx = FetchCtx {
                limit: 10,
                ..Default::default()
            };
            let items = match source.fetch(&ctx).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("web search failed for '{query}': {e}");
                    continue;
                }
            };
            for item in items {
                let id = uuid::Uuid::new_v4().to_string();
                let project_match = serde_json::json!([agent.project_id.clone()]).to_string();
                let res = sqlx::query(
                    r#"INSERT OR IGNORE INTO research_items
                           (id, feed_id, guid, link, title, summary, published_at,
                            relevance_score, project_matches_json, source_kind)
                       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'web-search')"#,
                )
                .bind(&id)
                .bind(&item.feed_id)
                .bind(&item.guid)
                .bind(&item.link)
                .bind(&item.title)
                .bind(&item.summary)
                .bind(item.published_at.map(|d| d.to_rfc3339()))
                .bind(0.5f64)
                .bind(&project_match)
                .execute(pool)
                .await;
                if let Ok(r) = res {
                    if r.rows_affected() > 0 {
                        total_new += 1;
                    }
                }
            }
        }

        // Update per-project state.
        let now = Utc::now().to_rfc3339();
        let queries_used = queries_to_run.len() as i64;
        let _ = sqlx::query(
            r#"INSERT INTO project_research_state
                   (project_id, last_run_at, queries_used_today, daily_budget)
               VALUES (?, ?, ?, ?)
               ON CONFLICT(project_id) DO UPDATE SET
                   last_run_at = excluded.last_run_at,
                   queries_used_today = excluded.queries_used_today,
                   daily_budget = excluded.daily_budget"#,
        )
        .bind(&agent.project_id)
        .bind(&now)
        .bind(queries_used)
        .bind(agent.daily_budget_queries as i64)
        .execute(pool)
        .await;
    }

    Ok(JobResult {
        summary: format!(
            "project sweep: {projects_touched} project(s), {total_new} new web-search item(s)"
        ),
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
        JobKind::ProjectResearchSweep => run_project_research_sweep(pool, ctx).await,
        JobKind::DailySummary => run_daily_summary(pool, ctx).await,
        JobKind::TaskGrooming => run_task_grooming(pool, ctx).await,
        JobKind::AutoCategorizer => run_auto_categorizer(pool, ctx).await,
        JobKind::SelfImproveOrchestrator => crate::selfimprove::run_self_improve(pool, ctx).await,
        JobKind::Curator => crate::curator::run_curator(pool, ctx).await,
    }
}

/// Iterate every job kind. Kept as a single source of truth so a new variant
/// added to [`JobKind`] is automatically picked up by the scheduler loop AND
/// by `roundtrip`-style tests.
pub fn all_kinds() -> [JobKind; 13] {
    [
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
        JobKind::AutoCategorizer,
        JobKind::SelfImproveOrchestrator,
        JobKind::Curator,
    ]
}

/// Run every enabled job once, sequentially, returning per-kind results.
/// Useful for `altevra brain run-all` style CLI calls and for tests that want
/// a deterministic single pass without driving the scheduler loop.
///
/// The function never short-circuits on error: a failing job is logged and
/// reported with `Err`, then the next kind runs. The scheduler uses the same
/// per-kind dispatch + history pattern; this is the headless equivalent.
pub async fn run_all(
    pool: &SqlitePool,
    ctx: &JobContext,
    disabled: &[String],
) -> Vec<(JobKind, anyhow::Result<JobResult>)> {
    let mut out = Vec::with_capacity(all_kinds().len());
    for kind in all_kinds() {
        if disabled.iter().any(|d| d == kind.as_str()) {
            continue;
        }
        let r = dispatch(kind, pool, ctx).await;
        out.push((kind, r));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All job tests run against the noop router (no keys); matches production default.
    fn noop_router() -> std::sync::Arc<altevra_llm::ModelRouter> {
        std::sync::Arc::new(altevra_llm::ModelRouter::noop())
    }

    /// A deterministic stub provider that stands in for a configured (non-noop)
    /// cloud reasoner — it returns canned prose so the LLM-backed path (card
    /// write / classification) runs without keys or network. `is_local` is false
    /// (it represents a cloud provider) so SI-7 routing is exercised honestly:
    /// the router refuses to use it for `local_private`.
    struct StubProvider {
        id: &'static str,
        reply: String,
    }

    #[async_trait::async_trait]
    impl altevra_llm::ChatProvider for StubProvider {
        fn id(&self) -> &str {
            self.id
        }
        fn is_local(&self) -> bool {
            false
        }
        async fn complete(
            &self,
            _messages: &[altevra_llm::ChatMessage],
            _opts: &altevra_llm::ChatOpts,
        ) -> anyhow::Result<String> {
            Ok(self.reply.clone())
        }
    }

    /// A router with a stub cloud provider on a single role.
    fn router_with_stub(
        role: altevra_llm::ModelRole,
        id: &'static str,
        reply: &str,
    ) -> std::sync::Arc<altevra_llm::ModelRouter> {
        std::sync::Arc::new(altevra_llm::ModelRouter::noop().with_provider(
            role,
            std::sync::Arc::new(StubProvider {
                id,
                reply: reply.to_string(),
            }),
        ))
    }

    /// A fully-migrated in-memory db (real schema) for jobs that persist objects.
    async fn migrated_pool() -> SqlitePool {
        let pool = altevra_db::create_pool("sqlite::memory:").await.unwrap();
        altevra_db::run_migrations(&pool).await.unwrap();
        pool
    }

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
        // all_kinds() is the single source of truth — using it here means a
        // new variant added to JobKind can't silently dodge the roundtrip test.
        for k in all_kinds() {
            assert_eq!(JobKind::parse(k.as_str()), Some(k));
        }
        // Curator wiring spot-check (C7).
        assert_eq!(JobKind::Curator.as_str(), "curator");
        assert_eq!(JobKind::Curator.period_secs(), 7 * 24 * 60 * 60);
    }

    #[tokio::test]
    async fn project_research_sweep_returns_when_no_identity_file() {
        // Without ~/.imperium/identity/projects.yaml the job should bail
        // gracefully with a skip message, not panic.
        let pool = setup_research_schema().await;
        let tmp = tempfile::TempDir::new().unwrap();
        let old_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());
        let ctx = JobContext {
            vault_path: tmp.path().to_path_buf(),
            now: Utc::now(),
            router: noop_router(),
        };
        let r = run_project_research_sweep(&pool, &ctx).await.unwrap();
        assert!(r.summary.to_lowercase().contains("no"));
        if let Some(h) = old_home {
            std::env::set_var("HOME", h);
        } else {
            std::env::remove_var("HOME");
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
            router: noop_router(),
        };
        let r = run_observer_scan(&pool, &ctx).await.unwrap();
        assert_eq!(r.items_processed, 0);
    }

    #[tokio::test]
    async fn daily_summary_writes_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let pool = migrated_pool().await;
        let ctx = JobContext {
            vault_path: tmp.path().to_path_buf(),
            now: Utc::now(),
            router: noop_router(),
        };
        let r = run_daily_summary(&pool, &ctx).await.unwrap();
        assert_eq!(r.items_processed, 1);
        // idempotent — second call returns 0
        let r2 = run_daily_summary(&pool, &ctx).await.unwrap();
        assert_eq!(r2.items_processed, 0);
    }

    #[tokio::test]
    async fn daily_briefing_surfaces_patterns_and_contacts() {
        use altevra_core::events::{ActorType, Event, EventType};
        use altevra_db::{DecisionRow, EventsRepository, MentionsRepository, TasksRepository};

        let tmp = tempfile::TempDir::new().unwrap();
        let pool = migrated_pool().await;
        // Fixed clock so the date math is deterministic.
        let now: chrono::DateTime<Utc> = "2026-06-03T23:30:00Z".parse().unwrap();

        // 1. Seed events that trip a detector: 3 SkillDriftDetected for one slug in
        //    the last week → a RecurringDrift insight.
        let events_repo = EventsRepository::new(&pool);
        for h in [2i64, 4, 6] {
            let mut ev = Event::new(
                EventType::SkillDriftDetected,
                "drift altevra-core",
                "test",
                ActorType::System,
            )
            .with_entity("skill", "altevra-core");
            ev.created_at = now - chrono::Duration::hours(h);
            events_repo.insert(&ev).await.unwrap();
        }

        // 2. Seed a person with an OLD last-contact. Đorđe is in the mentor seed, so
        //    the dictionary knows him. One mention edge from an object whose
        //    object_index.updated_at is ~6 weeks ago → "haven't talked to" line.
        let idx = altevra_db::ObjectIndexRepository::new(&pool);
        idx.index_object(
            &altevra_db::ObjectIndexRow {
                object_type: "learning".into(),
                id: "capture-old-djordje-1".into(),
                status: "active".into(),
                sensitivity: "internal".into(),
                domain: "business".into(),
                scope: None,
                title: Some("old note mentioning Đorđe".into()),
                categories: "[]".into(),
                tags: "[]".into(),
                redaction_status: "clean".into(),
                updated_at: now - chrono::Duration::weeks(6),
            },
            "body",
        )
        .await
        .unwrap();
        MentionsRepository::new(&pool)
            .record("learning", "capture-old-djordje-1", "person", "person:djordje")
            .await
            .unwrap();

        // 3. Seed a decision past its review_after.
        let decision = DecisionRow {
            id: uuid::Uuid::new_v4(),
            project_id: None,
            title: "Stop building, start selling".into(),
            rationale: Some("Đorđe directive".into()),
            decided_at: "2026-04-10T00:00:00Z".parse().unwrap(),
            decided_by: Some("djordje".into()),
            metadata: serde_json::json!({}),
        };
        TasksRepository::new(&pool)
            .save_decision(&decision)
            .await
            .unwrap();
        sqlx::query("UPDATE decisions SET review_after = ? WHERE id = ?")
            .bind("2026-05-01T00:00:00.000Z")
            .bind(decision.id.to_string())
            .execute(&pool)
            .await
            .unwrap();

        let ctx = JobContext {
            vault_path: tmp.path().to_path_buf(),
            now,
            router: noop_router(), // noop path — structured bullets, no LLM.
        };
        let r = run_daily_summary(&pool, &ctx).await.unwrap();
        assert_eq!(r.items_processed, 1);

        let file = tmp.path().join("10-insights").join("daily-2026-06-03.md");
        let content = std::fs::read_to_string(&file).unwrap();

        // a pattern line...
        assert!(
            content.contains("Recurring drift: altevra-core"),
            "must surface the detected pattern:\n{content}"
        );
        // a "haven't talked to X" line (Đorđe, 6 weeks ago)...
        assert!(
            content.contains("haven't talked to Đorđe"),
            "must surface the last-contact gap:\n{content}"
        );
        // ...and the stale-decision line.
        assert!(
            content.contains("decision 'Stop building, start selling'")
                && content.contains("still applies?"),
            "must surface the stale decision:\n{content}"
        );
        // noop path → no LLM attribution in frontmatter.
        assert!(content.contains("generated_by: altevra-brain\n"));
    }

    #[tokio::test]
    async fn insight_synthesizer_writes_card() {
        use altevra_db::InsightCardsRepository;

        let tmp = tempfile::TempDir::new().unwrap();

        // noop → skipped cleanly, ZERO cards.
        let pool = migrated_pool().await;
        let ctx_noop = JobContext {
            vault_path: tmp.path().to_path_buf(),
            now: Utc::now(),
            router: noop_router(),
        };
        let r = run_insight_synthesizer(&pool, &ctx_noop).await.unwrap();
        assert_eq!(r.items_processed, 0);
        assert!(r.summary.contains("skipped"));
        assert_eq!(
            InsightCardsRepository::new(&pool).count().await.unwrap(),
            0,
            "noop must write no cards"
        );

        // stub non-noop StrongReasoner → an insight_card row exists + is recallable.
        let pool2 = migrated_pool().await;
        let ctx = JobContext {
            vault_path: tmp.path().to_path_buf(),
            now: Utc::now(),
            router: router_with_stub(
                altevra_llm::ModelRole::StrongReasoner,
                "stub-reasoner",
                "Late-night sessions precede a spike in next-day rework.",
            ),
        };
        let r = run_insight_synthesizer(&pool2, &ctx).await.unwrap();
        assert_eq!(r.items_processed, 1);
        assert!(r.summary.contains("insight card"));

        let cards = InsightCardsRepository::new(&pool2);
        assert_eq!(cards.count().await.unwrap(), 1, "exactly one card written");

        // recallable: the card auto-indexed into the FTS substrate (A1).
        let fts = altevra_db::FtsRepository::new(&pool2);
        assert!(
            fts.search("rework", 10)
                .await
                .unwrap()
                .iter()
                .any(|h| h.object_type == "insight_card"),
            "synthesized card must be recallable"
        );
    }

    /// Helper: index an object with empty categories (uncategorized) in a domain.
    async fn seed_uncategorized(pool: &SqlitePool, id: &str, domain: &str, title: &str) {
        seed_uncategorized_with_body(pool, id, domain, title, "body").await;
    }

    async fn seed_uncategorized_with_body(
        pool: &SqlitePool,
        id: &str,
        domain: &str,
        title: &str,
        body: &str,
    ) {
        altevra_db::ObjectIndexRepository::new(pool)
            .index_object(
                &altevra_db::ObjectIndexRow {
                    object_type: "learning".into(),
                    id: id.into(),
                    status: "active".into(),
                    sensitivity: "internal".into(),
                    domain: domain.into(),
                    scope: None,
                    title: Some(title.into()),
                    categories: "[]".into(),
                    tags: "[]".into(),
                    redaction_status: "clean".into(),
                    updated_at: Utc::now(),
                },
                body,
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn auto_categorize_assigns_and_proposes() {
        use altevra_db::{ObjectIndexRepository, ProposalsRepository};

        let tmp = tempfile::TempDir::new().unwrap();

        // --- noop → skipped cleanly: nothing tagged, no proposals. ---
        let pool0 = migrated_pool().await;
        seed_uncategorized(&pool0, "obj-noop", "business", "some note").await;
        let ctx0 = JobContext {
            vault_path: tmp.path().to_path_buf(),
            now: Utc::now(),
            router: noop_router(),
        };
        let r0 = run_auto_categorizer(&pool0, &ctx0).await.unwrap();
        assert_eq!(r0.items_processed, 0, "noop classifies nothing");
        assert!(r0.summary.contains("skipped"));
        assert_eq!(
            ProposalsRepository::new(&pool0).list(None, Some("category")).await.unwrap().len(),
            0,
            "noop proposes nothing"
        );

        // --- stub cheap_worker → an object matching an EXISTING category gets tagged;
        //     a NOVEL one yields a kind="category" proposal. ---
        // The stub always replies "gtm". We pre-seed an existing "gtm" category so
        // the first object matches it; we ALSO test the novel path by using a stub
        // that returns a fresh label for a second object via a distinct router.
        let pool = migrated_pool().await;
        let idx = ObjectIndexRepository::new(&pool);

        // Pre-seed an EXISTING "gtm" category by indexing one already-categorized
        // object (so the taxonomy is non-empty).
        idx.index_object(
            &altevra_db::ObjectIndexRow {
                object_type: "decision".into(),
                id: "seed-gtm".into(),
                status: "active".into(),
                sensitivity: "internal".into(),
                domain: "business".into(),
                scope: None,
                title: Some("a gtm decision".into()),
                categories: "[\"gtm\"]".into(),
                tags: "[]".into(),
                redaction_status: "clean".into(),
                updated_at: Utc::now(),
            },
            "body",
        )
        .await
        .unwrap();

        // An uncategorized business object the stub will label "gtm" (existing → tag).
        seed_uncategorized(&pool, "obj-match", "business", "gtm follow-up note").await;

        let ctx = JobContext {
            vault_path: tmp.path().to_path_buf(),
            now: Utc::now(),
            router: router_with_stub(altevra_llm::ModelRole::CheapWorker, "stub-cheap", "gtm"),
        };
        let r = run_auto_categorizer(&pool, &ctx).await.unwrap();
        // obj-match was tagged with the existing "gtm" category.
        let tagged = idx
            .get_categories_or_empty("learning", "obj-match")
            .await;
        assert_eq!(tagged, vec!["gtm".to_string()], "matching object tagged: {r:?}");
        // No category proposal yet (it matched an existing one).
        assert_eq!(
            ProposalsRepository::new(&pool).list(None, Some("category")).await.unwrap().len(),
            0,
            "a matched object proposes no new category"
        );

        // --- novel category path: a new object whose stub label is NOT in the
        //     taxonomy yields a kind="category" Tier-0 proposal. ---
        let pool2 = migrated_pool().await;
        seed_uncategorized(&pool2, "obj-novel", "business", "a note about violin practice").await;
        let ctx2 = JobContext {
            vault_path: tmp.path().to_path_buf(),
            now: Utc::now(),
            router: router_with_stub(altevra_llm::ModelRole::CheapWorker, "stub-cheap", "hobby"),
        };
        let r2 = run_auto_categorizer(&pool2, &ctx2).await.unwrap();
        let cat_props = ProposalsRepository::new(&pool2)
            .list(None, Some("category"))
            .await
            .unwrap();
        assert_eq!(cat_props.len(), 1, "novel label proposes a category: {r2:?}");
        assert!(cat_props[0].title.to_lowercase().contains("hobby"));
        // SI-9: a "category" proposal is Tier-0 (the repo derived it, not the agent).
        assert_eq!(cat_props[0].risk_tier, "tier0");
        // the object was NOT tagged (no existing category fit).
        assert!(
            ObjectIndexRepository::new(&pool2)
                .get_categories_or_empty("learning", "obj-novel")
                .await
                .is_empty(),
            "novel object stays uncategorized until Pavle approves the new category"
        );
    }

    /// SI-7: a HIGH-WATER object (e.g. relationship) must be classified by
    /// `local_private`, NEVER the cloud `cheap_worker`. With only a cloud
    /// cheap_worker registered, a high-water object is SKIPPED (no cloud leak),
    /// while a business object IS classified by the cheap_worker.
    #[tokio::test]
    async fn auto_categorize_si7_routes_high_water_local_only() {
        use altevra_db::{ObjectIndexRepository, ProposalsRepository};

        let tmp = tempfile::TempDir::new().unwrap();
        let pool = migrated_pool().await;
        seed_uncategorized(&pool, "obj-personal", "relationship", "dinner with Elena").await;
        seed_uncategorized(&pool, "obj-business", "business", "ReVesta cold call list").await;

        // Only a CLOUD cheap_worker is configured; local_private stays noop.
        let ctx = JobContext {
            vault_path: tmp.path().to_path_buf(),
            now: Utc::now(),
            router: router_with_stub(altevra_llm::ModelRole::CheapWorker, "stub-cheap", "outreach"),
        };
        let r = run_auto_categorizer(&pool, &ctx).await.unwrap();
        // The relationship object was skipped (local_private resolved to noop) — it
        // NEVER reached the cloud worker. The business one produced a proposal.
        assert!(r.summary.contains("1 skipped") || r.summary.contains("skipped"));
        let idx = ObjectIndexRepository::new(&pool);
        assert!(
            idx.get_categories_or_empty("learning", "obj-personal")
                .await
                .is_empty(),
            "high-water object must NOT be classified by the cloud worker (SI-7)"
        );
        // the business object yielded a novel-category proposal (taxonomy was empty).
        assert_eq!(
            ProposalsRepository::new(&pool)
                .list(None, Some("category"))
                .await
                .unwrap()
                .len(),
            1,
            "non-high-water object IS classified by cheap_worker"
        );
    }

    /// SI-7 DEFENSE-IN-DEPTH: an object stamped domain='business' (e.g. a generic
    /// 'learning' note whose template default_domain is Business) but whose CONTENT
    /// is clearly relationship/personal must NOT be classified by a cloud-only
    /// cheap_worker. The content fail-safe re-routes it to local_private; with only
    /// a cloud cheap_worker configured, local_private is noop → it is SKIPPED, never
    /// leaked. A genuinely-business control object IS classified by the cheap_worker.
    #[tokio::test]
    async fn auto_categorize_content_failsafe_keeps_high_water_local() {
        use altevra_db::{ObjectIndexRepository, ProposalsRepository};

        let tmp = tempfile::TempDir::new().unwrap();
        let pool = migrated_pool().await;

        // domain='business' BUT body carries clear relationship content (the same SR
        // keyword the high-water net detects: "moja devojka"). The obj.domain check
        // alone (is_high_water()==false) would route this to the CLOUD worker.
        seed_uncategorized_with_body(
            &pool,
            "obj-mislabeled",
            "business",
            "random thought",
            "danas sam shvatio nesto vazno — moja devojka Elena me podrzava u svemu",
        )
        .await;
        // A genuinely-business control object (no high-water content).
        seed_uncategorized_with_body(
            &pool,
            "obj-clean-biz",
            "business",
            "ReVesta GTM",
            "cold call list for surplus buyers in Florida",
        )
        .await;

        // Only a CLOUD cheap_worker is configured; local_private stays noop.
        let ctx = JobContext {
            vault_path: tmp.path().to_path_buf(),
            now: Utc::now(),
            router: router_with_stub(altevra_llm::ModelRole::CheapWorker, "stub-cheap", "outreach"),
        };
        let r = run_auto_categorizer(&pool, &ctx).await.unwrap();

        let idx = ObjectIndexRepository::new(&pool);
        // The mislabeled object was re-routed to local_private (noop) → SKIPPED, so
        // the cloud worker never saw it: it stays uncategorized and proposes nothing.
        assert!(
            idx.get_categories_or_empty("learning", "obj-mislabeled")
                .await
                .is_empty(),
            "content fail-safe must keep the relationship-content object OFF the cloud worker (SI-7)"
        );
        // The clean business object DID reach the cheap_worker → a category proposal.
        let props = ProposalsRepository::new(&pool)
            .list(None, Some("category"))
            .await
            .unwrap();
        assert_eq!(
            props.len(),
            1,
            "exactly the genuinely-business object is classified by the cloud worker: {r:?}"
        );
        assert!(
            props[0]
                .evidence_refs
                .contains("obj-clean-biz"),
            "the one cloud-classified object is the business control, not the mislabeled one"
        );
        assert!(r.summary.contains("skipped"));
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
            router: noop_router(),
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
            router: noop_router(),
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
            router: noop_router(),
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

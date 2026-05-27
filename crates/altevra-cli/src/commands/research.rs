use altevra_research::{
    discover::{extract_feed_links, extract_outbound_links, filter_promising_blog_links},
    feeds::{FeedConfig, FeedKind, FeedSource, ProjectKeywordsSource},
    fetcher::{fetch_feed, FetchCacheHints},
    relevance::{default_imperium_projects_path, load_imperium_projects, matching_projects},
    scrape_url, sources, synthesize, ResearchPipeline, ScoredItem, SynthesisInput,
};
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum ResearchCommands {
    /// Run full research pipeline (scrape + synthesize)
    Run(ResearchRunArgs),
    /// Scrape a single URL and print extracted content
    Scrape(ResearchScrapeArgs),
    /// Synthesize already-scraped content from JSON file
    Synthesize(ResearchSynthesizeArgs),
    /// Manage RSS/Atom feeds
    #[command(subcommand)]
    Feeds(FeedsCommands),
    /// Fetch all enabled feeds once, score and write briefs
    RunNow(RunNowArgs),
    /// Fetch GitHub Trending repos for a given language
    Trending(TrendingArgs),
}

#[derive(Subcommand)]
pub enum FeedsCommands {
    /// Create ~/.altevra/research/feeds.yaml with the default packet
    Init(FeedsInitArgs),
    /// Add a new feed source
    Add(FeedsAddArgs),
    /// List configured feeds
    List(FeedsListArgs),
    /// Remove a feed by id
    Remove(FeedsRemoveArgs),
    /// Scan a URL for RSS/Atom feed links and (optionally) auto-promote them
    Discover(FeedsDiscoverArgs),
    /// List discovered feed candidates from the brain's discovery queue
    Candidates(FeedsCandidatesArgs),
    /// Promote a discovered candidate into the active feeds.yaml
    Promote(FeedsPromoteArgs),
    /// Reject a discovered candidate (won't be auto-added on future runs)
    Reject(FeedsRejectArgs),
}

#[derive(Args)]
pub struct FeedsDiscoverArgs {
    pub url: String,
    /// Add discovered feeds to feeds.yaml immediately
    #[arg(long)]
    pub auto_promote: bool,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct FeedsCandidatesArgs {
    /// Filter by status: pending | promoted | rejected
    #[arg(long, default_value = "pending")]
    pub status: String,
    #[arg(long, default_value_t = 50)]
    pub limit: i64,
    #[arg(long, default_value = ".altevra/altevra.db")]
    pub db: std::path::PathBuf,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct FeedsPromoteArgs {
    pub candidate_id: String,
    #[arg(long, default_value_t = 0.5)]
    pub trust_weight: f32,
    #[arg(long, default_value = ".altevra/altevra.db")]
    pub db: std::path::PathBuf,
}

#[derive(Args)]
pub struct FeedsRejectArgs {
    pub candidate_id: String,
    #[arg(long)]
    pub reason: Option<String>,
    #[arg(long, default_value = ".altevra/altevra.db")]
    pub db: std::path::PathBuf,
}

#[derive(Args)]
pub struct TrendingArgs {
    /// Language slug: rust, typescript, python (None = all languages)
    #[arg(long)]
    pub lang: Option<String>,
    /// Time window: daily | weekly | monthly
    #[arg(long, default_value = "daily")]
    pub since: String,
    #[arg(long, default_value_t = 25)]
    pub limit: usize,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct ResearchRunArgs {
    pub query: String,
    #[arg(long, num_args = 1.., value_delimiter = ',')]
    pub urls: Vec<String>,
    #[arg(long)]
    pub vault: Option<PathBuf>,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct ResearchScrapeArgs {
    pub url: String,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct ResearchSynthesizeArgs {
    pub query: String,
    #[arg(long)]
    pub pages_file: PathBuf,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct FeedsInitArgs {
    /// Use the built-in default packet (~30 sources)
    #[arg(long)]
    pub default_packet: bool,
    /// Overwrite existing feeds.yaml
    #[arg(long)]
    pub force: bool,
    /// Override output path (defaults to ~/.altevra/research/feeds.yaml)
    #[arg(long)]
    pub path: Option<PathBuf>,
}

#[derive(Args)]
pub struct FeedsAddArgs {
    pub url: String,
    #[arg(long)]
    pub id: Option<String>,
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long, default_value = "general")]
    pub category: String,
    #[arg(long, default_value_t = 0.7)]
    pub trust_weight: f32,
    #[arg(long, default_value_t = 180)]
    pub interval_minutes: u32,
    #[arg(long)]
    pub path: Option<PathBuf>,
}

#[derive(Args)]
pub struct FeedsListArgs {
    #[arg(long)]
    pub enabled_only: bool,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub path: Option<PathBuf>,
}

#[derive(Args)]
pub struct FeedsRemoveArgs {
    pub id: String,
    #[arg(long)]
    pub path: Option<PathBuf>,
}

#[derive(Args)]
pub struct RunNowArgs {
    /// Optional override path to feeds.yaml
    #[arg(long)]
    pub feeds_file: Option<PathBuf>,
    /// Time window for items (in days)
    #[arg(long)]
    pub window_days: Option<u32>,
    /// Vault root for per-project briefs
    #[arg(long, default_value = ".")]
    pub vault: PathBuf,
    /// Don't write anything — just print what would happen
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long)]
    pub json: bool,
}

pub async fn run(cmd: ResearchCommands) -> anyhow::Result<()> {
    match cmd {
        ResearchCommands::Run(args) => run_full(args).await,
        ResearchCommands::Scrape(args) => run_scrape(args).await,
        ResearchCommands::Synthesize(args) => run_synthesize(args).await,
        ResearchCommands::Feeds(cmd) => run_feeds(cmd).await,
        ResearchCommands::RunNow(args) => run_now(args).await,
        ResearchCommands::Trending(args) => run_trending(args).await,
    }
}

async fn run_full(args: ResearchRunArgs) -> anyhow::Result<()> {
    let pipeline = ResearchPipeline::new(args.vault);
    let result = pipeline.run(&args.query, &args.urls).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!("Research: {}", result.query);
        println!("  Pages scraped: {}", result.pages.len());
        if let Some(p) = &result.saved_path {
            println!("  Saved to: {}", p.display());
        }
        println!("\n{}", result.synthesis);
    }
    Ok(())
}

async fn run_scrape(args: ResearchScrapeArgs) -> anyhow::Result<()> {
    let page = scrape_url(&args.url).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&page)?);
    } else {
        println!("URL: {}", page.url);
        println!("Status: {}", page.status);
        if let Some(t) = &page.title {
            println!("Title: {t}");
        }
        println!("\n{}", page.text);
    }
    Ok(())
}

async fn run_synthesize(args: ResearchSynthesizeArgs) -> anyhow::Result<()> {
    let raw = std::fs::read_to_string(&args.pages_file)?;
    let pages: Vec<altevra_research::ScrapedPage> = serde_json::from_str(&raw)?;
    let synthesis = synthesize(SynthesisInput {
        query: &args.query,
        pages: &pages,
    });
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "query": args.query,
                "synthesis": synthesis,
                "pages": pages.len(),
            }))?
        );
    } else {
        println!("{synthesis}");
    }
    Ok(())
}

// ---- Feeds subcommands ------------------------------------------------------

async fn run_feeds(cmd: FeedsCommands) -> anyhow::Result<()> {
    match cmd {
        FeedsCommands::Init(args) => run_feeds_init(args).await,
        FeedsCommands::Add(args) => run_feeds_add(args).await,
        FeedsCommands::List(args) => run_feeds_list(args).await,
        FeedsCommands::Remove(args) => run_feeds_remove(args).await,
        FeedsCommands::Discover(args) => run_feeds_discover(args).await,
        FeedsCommands::Candidates(args) => run_feeds_candidates(args).await,
        FeedsCommands::Promote(args) => run_feeds_promote(args).await,
        FeedsCommands::Reject(args) => run_feeds_reject(args).await,
    }
}

async fn run_feeds_discover(args: FeedsDiscoverArgs) -> anyhow::Result<()> {
    // Reuse the existing scrape helper (which already does reqwest behind the scenes)
    // — we only need the HTML body here, not the readable text.
    let page = scrape_url(&args.url).await?;
    let status = page.status;
    let html = &page.html;
    let feed_links = extract_feed_links(&args.url, html);
    let outbound = extract_outbound_links(&args.url, html);
    let promising = filter_promising_blog_links(&outbound);

    let mut promoted_count = 0;
    if args.auto_promote {
        let cfg_path = FeedConfig::default_path();
        let mut cfg = if cfg_path.exists() {
            FeedConfig::load(&cfg_path)?
        } else {
            altevra_research::default_feeds()
        };
        for f in &feed_links {
            let id = slugify_url(f);
            if cfg.find(&id).is_some() {
                continue;
            }
            let _ = cfg.add(FeedSource {
                id,
                name: f.clone(),
                url: f.clone(),
                kind: FeedKind::Rss,
                category: "auto-discovered".into(),
                trust_weight: 0.5,
                enabled: true,
                fetch_interval_minutes: 180,
            });
            promoted_count += 1;
        }
        cfg.save(&cfg_path)?;
    }

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "url": args.url,
                "status": status,
                "feed_links": feed_links,
                "promising_outbound": promising,
                "auto_promote": args.auto_promote,
                "promoted_count": promoted_count,
            }))?
        );
    } else {
        println!("Status: {status}");
        println!("Feed links ({}):", feed_links.len());
        for f in &feed_links {
            println!("  {f}");
        }
        if !promising.is_empty() {
            println!("Promising outbound ({}):", promising.len());
            for l in promising.iter().take(10) {
                println!("  {l}");
            }
        }
        if args.auto_promote {
            println!("Promoted {promoted_count} new feed(s) into feeds.yaml");
        }
    }
    Ok(())
}

async fn run_feeds_candidates(args: FeedsCandidatesArgs) -> anyhow::Result<()> {
    let pool = altevra_db::create_pool(&args.db.to_string_lossy()).await?;
    altevra_db::run_migrations(&pool).await?;
    let rows = sqlx::query(
        r#"SELECT id, candidate_url, feed_url, source_url, discovered_at, status, discovered_by
           FROM research_feed_candidates
           WHERE status = ?
           ORDER BY discovered_at DESC LIMIT ?"#,
    )
    .bind(&args.status)
    .bind(args.limit)
    .fetch_all(&pool)
    .await?;

    let entries: Vec<_> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": sqlx::Row::try_get::<String, _>(r, "id").unwrap_or_default(),
                "candidate_url": sqlx::Row::try_get::<String, _>(r, "candidate_url").unwrap_or_default(),
                "feed_url": sqlx::Row::try_get::<Option<String>, _>(r, "feed_url").unwrap_or(None),
                "source_url": sqlx::Row::try_get::<Option<String>, _>(r, "source_url").unwrap_or(None),
                "discovered_at": sqlx::Row::try_get::<String, _>(r, "discovered_at").unwrap_or_default(),
                "discovered_by": sqlx::Row::try_get::<Option<String>, _>(r, "discovered_by").unwrap_or(None),
                "status": sqlx::Row::try_get::<String, _>(r, "status").unwrap_or_default(),
            })
        })
        .collect();
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "status": args.status,
                "count": entries.len(),
                "candidates": entries,
            }))?
        );
    } else if entries.is_empty() {
        println!("No candidates with status '{}'", args.status);
    } else {
        println!("{} candidate(s) [{}]:", entries.len(), args.status);
        for e in &entries {
            println!(
                "  {} {} (from {})",
                &e["id"].as_str().unwrap_or("")[..8],
                e["candidate_url"].as_str().unwrap_or(""),
                e["source_url"].as_str().unwrap_or("-")
            );
        }
    }
    Ok(())
}

async fn run_feeds_promote(args: FeedsPromoteArgs) -> anyhow::Result<()> {
    let pool = altevra_db::create_pool(&args.db.to_string_lossy()).await?;
    altevra_db::run_migrations(&pool).await?;
    let row =
        sqlx::query("SELECT candidate_url, feed_url FROM research_feed_candidates WHERE id = ?")
            .bind(&args.candidate_id)
            .fetch_optional(&pool)
            .await?
            .ok_or_else(|| anyhow::anyhow!("no candidate with id {}", args.candidate_id))?;
    let candidate_url: String = sqlx::Row::try_get(&row, "candidate_url")?;
    let feed_url: Option<String> = sqlx::Row::try_get(&row, "feed_url").ok();
    let target_url = feed_url.unwrap_or(candidate_url.clone());

    let cfg_path = FeedConfig::default_path();
    let mut cfg = if cfg_path.exists() {
        FeedConfig::load(&cfg_path)?
    } else {
        altevra_research::default_feeds()
    };
    let id = slugify_url(&target_url);
    if cfg.find(&id).is_none() {
        cfg.add(FeedSource {
            id: id.clone(),
            name: target_url.clone(),
            url: target_url.clone(),
            kind: FeedKind::Rss,
            category: "auto-discovered".into(),
            trust_weight: args.trust_weight,
            enabled: true,
            fetch_interval_minutes: 180,
        })?;
        cfg.save(&cfg_path)?;
    }

    sqlx::query(
        "UPDATE research_feed_candidates SET status = 'promoted', auto_promoted_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?",
    )
    .bind(&args.candidate_id)
    .execute(&pool)
    .await?;

    println!("Promoted candidate {} -> feed '{id}'", args.candidate_id);
    Ok(())
}

async fn run_feeds_reject(args: FeedsRejectArgs) -> anyhow::Result<()> {
    let pool = altevra_db::create_pool(&args.db.to_string_lossy()).await?;
    altevra_db::run_migrations(&pool).await?;
    let reason = args.reason.unwrap_or_else(|| "manual reject".into());
    let n = sqlx::query(
        "UPDATE research_feed_candidates SET status = 'rejected', rejected_reason = ? WHERE id = ?",
    )
    .bind(&reason)
    .bind(&args.candidate_id)
    .execute(&pool)
    .await?
    .rows_affected();
    if n == 0 {
        anyhow::bail!("no candidate with id {}", args.candidate_id);
    }
    println!("Rejected candidate {} ({reason})", args.candidate_id);
    Ok(())
}

async fn run_trending(args: TrendingArgs) -> anyhow::Result<()> {
    let period = match args.since.as_str() {
        "weekly" => sources::github_trending::TrendingPeriod::Weekly,
        "monthly" => sources::github_trending::TrendingPeriod::Monthly,
        _ => sources::github_trending::TrendingPeriod::Daily,
    };
    let source = sources::github_trending::GitHubTrendingSource::new(args.lang.clone(), period);
    let ctx = sources::FetchCtx {
        window_days: 1,
        ..Default::default()
    };
    use sources::SourceProvider;
    let items = source.fetch(&ctx).await?;
    let limit = args.limit.min(items.len());
    let slice = &items[..limit];
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "language": args.lang,
                "since": args.since,
                "count": slice.len(),
                "items": slice,
            }))?
        );
    } else {
        println!(
            "GitHub Trending ({}{}, {}):",
            args.lang.as_deref().unwrap_or("all"),
            if args.lang.is_some() { "" } else { " langs" },
            args.since
        );
        for it in slice {
            println!("  {} — {}", it.title, it.link);
            if !it.summary.is_empty() {
                println!("    {}", truncate(&it.summary, 120));
            }
        }
    }
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

fn feeds_path(override_path: Option<PathBuf>) -> PathBuf {
    override_path.unwrap_or_else(FeedConfig::default_path)
}

async fn run_feeds_init(args: FeedsInitArgs) -> anyhow::Result<()> {
    let path = feeds_path(args.path);
    if path.exists() && !args.force {
        println!(
            "feeds.yaml already exists at {} (pass --force to overwrite)",
            path.display()
        );
        return Ok(());
    }
    let cfg = if args.default_packet {
        altevra_research::default_feeds()
    } else {
        FeedConfig {
            feeds: vec![],
            window_days: 7,
            relevance_threshold: 0.4,
            project_keywords_source: ProjectKeywordsSource::ImperiumIdentity,
            brief_paths: altevra_research::BriefPaths::default(),
        }
    };
    cfg.save(&path)?;
    println!("Wrote {} feeds to {}", cfg.feeds.len(), path.display());
    Ok(())
}

async fn run_feeds_add(args: FeedsAddArgs) -> anyhow::Result<()> {
    let path = feeds_path(args.path);
    let mut cfg = if path.exists() {
        FeedConfig::load(&path)?
    } else {
        FeedConfig {
            feeds: vec![],
            window_days: 7,
            relevance_threshold: 0.4,
            project_keywords_source: ProjectKeywordsSource::ImperiumIdentity,
            brief_paths: altevra_research::BriefPaths::default(),
        }
    };

    let id = args.id.clone().unwrap_or_else(|| slugify_url(&args.url));
    let name = args.name.clone().unwrap_or_else(|| id.clone());

    cfg.add(FeedSource {
        id: id.clone(),
        name,
        url: args.url,
        kind: FeedKind::Rss,
        category: args.category,
        trust_weight: args.trust_weight,
        enabled: true,
        fetch_interval_minutes: args.interval_minutes,
    })?;
    cfg.save(&path)?;
    println!("Added feed '{id}' to {}", path.display());
    Ok(())
}

async fn run_feeds_list(args: FeedsListArgs) -> anyhow::Result<()> {
    let path = feeds_path(args.path);
    if !path.exists() {
        if args.json {
            println!("[]");
        } else {
            println!(
                "No feeds.yaml at {} — run `altevra research feeds init --default-packet`",
                path.display()
            );
        }
        return Ok(());
    }
    let cfg = FeedConfig::load(&path)?;
    let feeds: Vec<_> = if args.enabled_only {
        cfg.enabled().cloned().collect()
    } else {
        cfg.feeds.clone()
    };
    if args.json {
        println!("{}", serde_json::to_string_pretty(&feeds)?);
    } else {
        println!("{} feed(s) in {}:", feeds.len(), path.display());
        for f in &feeds {
            let on = if f.enabled { "ON " } else { "off" };
            println!(
                "  [{on}] {:<28} {:<10} trust={:.2} every={}m  {}",
                f.id, f.category, f.trust_weight, f.fetch_interval_minutes, f.url
            );
        }
    }
    Ok(())
}

async fn run_feeds_remove(args: FeedsRemoveArgs) -> anyhow::Result<()> {
    let path = feeds_path(args.path);
    if !path.exists() {
        anyhow::bail!("no feeds.yaml at {}", path.display());
    }
    let mut cfg = FeedConfig::load(&path)?;
    if !cfg.remove(&args.id) {
        anyhow::bail!("no feed with id '{}'", args.id);
    }
    cfg.save(&path)?;
    println!("Removed feed '{}'", args.id);
    Ok(())
}

// ---- run-now ----------------------------------------------------------------

async fn run_now(args: RunNowArgs) -> anyhow::Result<()> {
    let cfg = if let Some(p) = &args.feeds_file {
        FeedConfig::load(p)?
    } else {
        FeedConfig::load_or_default()
    };
    let window = args.window_days.unwrap_or(cfg.window_days);
    let projects_path = default_imperium_projects_path();
    let projects = load_imperium_projects(&projects_path).unwrap_or_default();

    let mut results = Vec::new();
    let mut all_scored: Vec<ScoredItem> = Vec::new();
    let mut feeds_seen = 0;

    for feed in cfg.enabled() {
        feeds_seen += 1;
        let outcome = match fetch_feed(feed, window, &FetchCacheHints::default()).await {
            Ok(o) => o,
            Err(e) => {
                results.push(serde_json::json!({
                    "feed_id": feed.id,
                    "status": "error",
                    "error": e.to_string(),
                }));
                continue;
            }
        };
        let mut new_items = Vec::new();
        for item in outcome.items {
            let (score, matched) = matching_projects(&item, &projects, cfg.relevance_threshold);
            new_items.push(serde_json::json!({
                "title": item.title,
                "link": item.link,
                "score": score,
                "matched": matched,
            }));
            all_scored.push(ScoredItem {
                item,
                score,
                matched_projects: matched,
            });
        }
        results.push(serde_json::json!({
            "feed_id": feed.id,
            "status": outcome.status,
            "items": new_items.len(),
            "sample": new_items.into_iter().take(3).collect::<Vec<_>>(),
        }));
    }

    let mut briefs_paths = Vec::new();
    if !args.dry_run && !all_scored.is_empty() {
        if let Ok(p) =
            altevra_research::write_daily_brief(&cfg.brief_paths.daily_obsidian, &all_scored)
        {
            briefs_paths.push(
                serde_json::json!({"kind": "daily_obsidian", "path": p.display().to_string()}),
            );
        }
        let mut pids: Vec<String> = all_scored
            .iter()
            .flat_map(|i| i.matched_projects.iter().cloned())
            .collect();
        pids.sort();
        pids.dedup();
        for pid in &pids {
            if let Ok(Some(p)) = altevra_research::write_project_brief(
                &args.vault,
                &cfg.brief_paths.project_vault,
                pid,
                &all_scored,
            ) {
                briefs_paths.push(serde_json::json!({
                    "kind": "project",
                    "project_id": pid,
                    "path": p.display().to_string()
                }));
            }
        }
    }

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "feeds_seen": feeds_seen,
                "window_days": window,
                "dry_run": args.dry_run,
                "total_items": all_scored.len(),
                "feeds": results,
                "briefs": briefs_paths,
            }))?
        );
    } else {
        println!(
            "Fetched {feeds_seen} feeds, {} items (window={}d, dry_run={})",
            all_scored.len(),
            window,
            args.dry_run
        );
        for b in &briefs_paths {
            println!("  brief: {b}");
        }
    }
    Ok(())
}

fn slugify_url(url: &str) -> String {
    let host = url
        .splitn(4, '/')
        .nth(2)
        .unwrap_or("feed")
        .replace('.', "-");
    let now = chrono::Utc::now().timestamp() % 100_000;
    format!("{host}-{now}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn synthesize_from_empty_pages() {
        let tmp = TempDir::new().unwrap();
        let pages_file = tmp.path().join("pages.json");
        std::fs::write(&pages_file, "[]").unwrap();
        run_synthesize(ResearchSynthesizeArgs {
            query: "test".into(),
            pages_file,
            json: true,
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn feeds_init_with_default_packet() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("feeds.yaml");
        run_feeds_init(FeedsInitArgs {
            default_packet: true,
            force: false,
            path: Some(path.clone()),
        })
        .await
        .unwrap();
        assert!(path.exists());
        let cfg = FeedConfig::load(&path).unwrap();
        assert!(cfg.feeds.len() >= 28);
    }

    #[tokio::test]
    async fn feeds_init_then_add_then_remove() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("feeds.yaml");
        run_feeds_init(FeedsInitArgs {
            default_packet: false,
            force: false,
            path: Some(path.clone()),
        })
        .await
        .unwrap();
        run_feeds_add(FeedsAddArgs {
            url: "https://example.com/rss".into(),
            id: Some("ex".into()),
            name: Some("Example".into()),
            category: "test".into(),
            trust_weight: 0.5,
            interval_minutes: 60,
            path: Some(path.clone()),
        })
        .await
        .unwrap();
        let cfg = FeedConfig::load(&path).unwrap();
        assert!(cfg.find("ex").is_some());

        run_feeds_remove(FeedsRemoveArgs {
            id: "ex".into(),
            path: Some(path.clone()),
        })
        .await
        .unwrap();
        let cfg = FeedConfig::load(&path).unwrap();
        assert!(cfg.find("ex").is_none());
    }

    #[tokio::test]
    async fn feeds_init_idempotent_without_force() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("feeds.yaml");
        run_feeds_init(FeedsInitArgs {
            default_packet: true,
            force: false,
            path: Some(path.clone()),
        })
        .await
        .unwrap();
        let n1 = FeedConfig::load(&path).unwrap().feeds.len();
        // Second call with default_packet=false should NOT overwrite (no --force).
        run_feeds_init(FeedsInitArgs {
            default_packet: false,
            force: false,
            path: Some(path.clone()),
        })
        .await
        .unwrap();
        let n2 = FeedConfig::load(&path).unwrap().feeds.len();
        assert_eq!(n1, n2);
    }

    #[test]
    fn slugify_url_produces_ascii_id() {
        let s = slugify_url("https://news.ycombinator.com/rss");
        assert!(s.contains("news-ycombinator-com"));
        assert!(!s.contains('/'));
    }
}

use altevra_research::{
    feeds::{FeedConfig, FeedKind, FeedSource, ProjectKeywordsSource},
    fetcher::{fetch_feed, FetchCacheHints},
    relevance::{default_imperium_projects_path, load_imperium_projects, matching_projects},
    scrape_url, synthesize, ResearchPipeline, ScoredItem, SynthesisInput,
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
    }
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

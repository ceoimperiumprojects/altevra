pub mod briefs;
pub mod crawl_bridge;
pub mod discover;
pub mod feeds;
pub mod fetcher;
pub mod pipeline;
pub mod projects;
pub mod relevance;
pub mod saver;
pub mod scraper;
pub mod sources;
pub mod synthesis;

pub use briefs::{write_daily_brief, write_project_brief, ScoredItem};
pub use crawl_bridge::{
    crawl_via_imperium, crawl_with_login, imperium_crawl_spec, CrawlOpts, CrawlResult,
    ImperiumCrawlSpec,
};
pub use discover::{
    extract_feed_links, extract_outbound_links, extract_sitemap_url, filter_promising_blog_links,
};
pub use feeds::{
    default_feeds, BriefPaths, FeedConfig, FeedKind, FeedSource, ProjectKeywordsSource,
};
pub use fetcher::{fetch_feed, FeedItem, FetchCacheHints, FetchOutcome};
pub use pipeline::{ResearchPipeline, ResearchResult};
pub use projects::ProjectAgent;
pub use relevance::{
    default_imperium_projects_path, load_imperium_projects, matching_projects, score_item,
    ProjectKeywords,
};
pub use saver::save_research;
pub use scraper::{scrape_url, ScrapedPage};
pub use sources::{
    github_trending::{GitHubTrendingSource, TrendingPeriod},
    rss::RssSource,
    web_search::{WebSearchProviderKind, WebSearchSource},
    FetchCtx, SourceKind, SourceProvider,
};
pub use synthesis::{synthesize, SynthesisInput};

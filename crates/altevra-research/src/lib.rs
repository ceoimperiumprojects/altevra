pub mod briefs;
pub mod feeds;
pub mod fetcher;
pub mod pipeline;
pub mod relevance;
pub mod saver;
pub mod scraper;
pub mod synthesis;

pub use briefs::{write_daily_brief, write_project_brief, ScoredItem};
pub use feeds::{
    default_feeds, BriefPaths, FeedConfig, FeedKind, FeedSource, ProjectKeywordsSource,
};
pub use fetcher::{fetch_feed, FeedItem, FetchCacheHints, FetchOutcome};
pub use pipeline::{ResearchPipeline, ResearchResult};
pub use relevance::{
    default_imperium_projects_path, load_imperium_projects, matching_projects, score_item,
    ProjectKeywords,
};
pub use saver::save_research;
pub use scraper::{scrape_url, ScrapedPage};
pub use synthesis::{synthesize, SynthesisInput};

pub mod pipeline;
pub mod saver;
pub mod scraper;
pub mod synthesis;

pub use pipeline::{ResearchPipeline, ResearchResult};
pub use saver::save_research;
pub use scraper::{scrape_url, ScrapedPage};
pub use synthesis::{synthesize, SynthesisInput};

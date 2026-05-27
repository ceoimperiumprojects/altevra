use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    saver::save_research,
    scraper::{scrape_url, ScrapedPage},
    synthesis::{synthesize, SynthesisInput},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchResult {
    pub query: String,
    pub pages: Vec<ScrapedPage>,
    pub synthesis: String,
    pub saved_path: Option<PathBuf>,
}

pub struct ResearchPipeline {
    pub vault_root: Option<PathBuf>,
}

impl ResearchPipeline {
    pub fn new(vault_root: Option<PathBuf>) -> Self {
        Self { vault_root }
    }

    /// Scrape a set of URLs, synthesize, optionally save.
    pub async fn run(&self, query: &str, urls: &[String]) -> anyhow::Result<ResearchResult> {
        let mut pages = Vec::new();
        for url in urls {
            match scrape_url(url).await {
                Ok(p) => pages.push(p),
                Err(e) => eprintln!("warning: scrape {url} failed: {e}"),
            }
        }
        let synthesis = synthesize(SynthesisInput {
            query,
            pages: &pages,
        });
        let saved_path = if let Some(root) = &self.vault_root {
            Some(self.save(root, query, &synthesis)?)
        } else {
            None
        };
        Ok(ResearchResult {
            query: query.to_string(),
            pages,
            synthesis,
            saved_path,
        })
    }

    fn save(&self, root: &Path, query: &str, content: &str) -> anyhow::Result<PathBuf> {
        save_research(root, query, content)
    }
}

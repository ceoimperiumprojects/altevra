//! Feeds configuration — YAML-driven list of RSS/Atom sources.
//!
//! Default packet (~30 sources) covers AI labs, research, devtools, community.
//! Pavle's full 150+ source registry lives in his Obsidian vault and is not
//! touched by Altevra.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FeedKind {
    Rss,
    Atom,
    JsonFeed,
}

impl Default for FeedKind {
    fn default() -> Self {
        Self::Rss
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedSource {
    pub id: String,
    pub name: String,
    pub url: String,
    #[serde(rename = "type", default)]
    pub kind: FeedKind,
    #[serde(default)]
    pub category: String,
    #[serde(default = "default_trust")]
    pub trust_weight: f32,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_interval")]
    pub fetch_interval_minutes: u32,
}

fn default_trust() -> f32 {
    0.7
}
fn default_enabled() -> bool {
    true
}
fn default_interval() -> u32 {
    180
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BriefPaths {
    /// Absolute path to Obsidian Briefs folder.
    pub daily_obsidian: PathBuf,
    /// Relative subdirectory inside vault_root for per-project briefs.
    pub project_vault: PathBuf,
}

impl Default for BriefPaths {
    fn default() -> Self {
        Self {
            daily_obsidian: dirs_home()
                .join("Obsidian")
                .join("Imperium")
                .join("Content")
                .join("Research")
                .join("Briefs"),
            project_vault: PathBuf::from("05-research"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProjectKeywordsSource {
    ImperiumIdentity,
    VaultReadmes,
    Inline,
}

impl Default for ProjectKeywordsSource {
    fn default() -> Self {
        Self::ImperiumIdentity
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedConfig {
    pub feeds: Vec<FeedSource>,
    #[serde(default = "default_window")]
    pub window_days: u32,
    #[serde(default = "default_threshold")]
    pub relevance_threshold: f32,
    #[serde(default)]
    pub project_keywords_source: ProjectKeywordsSource,
    #[serde(default)]
    pub brief_paths: BriefPaths,
}

fn default_window() -> u32 {
    7
}
fn default_threshold() -> f32 {
    0.4
}

impl FeedConfig {
    /// Load feeds.yaml from a path. Returns the parsed config or error.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        let cfg: FeedConfig = serde_yaml::from_str(&raw)?;
        Ok(cfg)
    }

    /// Default location: `~/.altevra/research/feeds.yaml`.
    pub fn default_path() -> PathBuf {
        dirs_home()
            .join(".altevra")
            .join("research")
            .join("feeds.yaml")
    }

    /// Load from default path; if missing, fall back to the built-in default packet.
    pub fn load_or_default() -> Self {
        let p = Self::default_path();
        if p.exists() {
            if let Ok(cfg) = Self::load(&p) {
                return cfg;
            }
        }
        default_feeds()
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let yaml = serde_yaml::to_string(self)?;
        std::fs::write(path, yaml)?;
        Ok(())
    }

    pub fn enabled(&self) -> impl Iterator<Item = &FeedSource> {
        self.feeds.iter().filter(|f| f.enabled)
    }

    pub fn find(&self, id: &str) -> Option<&FeedSource> {
        self.feeds.iter().find(|f| f.id == id)
    }

    pub fn add(&mut self, source: FeedSource) -> anyhow::Result<()> {
        if self.feeds.iter().any(|f| f.id == source.id) {
            anyhow::bail!("feed with id '{}' already exists", source.id);
        }
        self.feeds.push(source);
        Ok(())
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.feeds.len();
        self.feeds.retain(|f| f.id != id);
        self.feeds.len() < before
    }
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

/// Built-in default packet — ~30 high-signal AI/dev/research feeds.
pub fn default_feeds() -> FeedConfig {
    let entries: &[(&str, &str, &str, &str, f32, u32)] = &[
        // (id, name, url, category, trust, interval_min)
        (
            "hn-frontpage",
            "Hacker News (Top)",
            "https://news.ycombinator.com/rss",
            "general",
            0.8,
            60,
        ),
        (
            "arxiv-cs-ai",
            "arXiv cs.AI",
            "https://arxiv.org/rss/cs.AI",
            "research",
            0.95,
            360,
        ),
        (
            "arxiv-cs-cl",
            "arXiv cs.CL",
            "https://arxiv.org/rss/cs.CL",
            "research",
            0.95,
            360,
        ),
        (
            "arxiv-cs-lg",
            "arXiv cs.LG",
            "https://arxiv.org/rss/cs.LG",
            "research",
            0.95,
            360,
        ),
        (
            "openai-news",
            "OpenAI News",
            "https://openai.com/blog/rss.xml",
            "official-ai",
            0.95,
            180,
        ),
        (
            "deepmind-blog",
            "DeepMind Blog",
            "https://deepmind.google/blog/rss.xml",
            "official-ai",
            0.95,
            360,
        ),
        (
            "anthropic-news",
            "Anthropic News",
            "https://www.anthropic.com/news/rss.xml",
            "official-ai",
            0.95,
            180,
        ),
        (
            "huggingface-blog",
            "Hugging Face Blog",
            "https://huggingface.co/blog/feed.xml",
            "official-ai",
            0.9,
            240,
        ),
        (
            "google-ai-blog",
            "Google AI Blog",
            "https://blog.research.google/feeds/posts/default",
            "official-ai",
            0.9,
            360,
        ),
        (
            "meta-ai-blog",
            "Meta AI Blog",
            "https://ai.meta.com/blog/rss/",
            "official-ai",
            0.9,
            360,
        ),
        (
            "cohere-blog",
            "Cohere Blog",
            "https://cohere.com/blog/rss.xml",
            "official-ai",
            0.85,
            360,
        ),
        (
            "lilianweng",
            "Lilian Weng",
            "https://lilianweng.github.io/index.xml",
            "research",
            0.95,
            1440,
        ),
        (
            "chiphuyen",
            "Chip Huyen",
            "https://huyenchip.com/feed.xml",
            "research",
            0.9,
            1440,
        ),
        (
            "thedecoder",
            "The Decoder",
            "https://the-decoder.com/feed/",
            "media",
            0.7,
            180,
        ),
        (
            "techcrunch-ai",
            "TechCrunch AI",
            "https://techcrunch.com/category/artificial-intelligence/feed/",
            "media",
            0.65,
            120,
        ),
        (
            "venturebeat-ai",
            "VentureBeat AI",
            "https://venturebeat.com/category/ai/feed/",
            "media",
            0.65,
            120,
        ),
        (
            "lobsters",
            "Lobsters",
            "https://lobste.rs/rss",
            "general",
            0.75,
            60,
        ),
        (
            "devto-ai",
            "Dev.to AI",
            "https://dev.to/feed/tag/ai",
            "community",
            0.55,
            240,
        ),
        (
            "github-trending-rust",
            "GitHub Trending Rust",
            "https://github.com/trending/rust.atom",
            "devtools",
            0.7,
            1440,
        ),
        (
            "bair-blog",
            "Berkeley AI Research",
            "https://bair.berkeley.edu/blog/feed.xml",
            "research",
            0.95,
            1440,
        ),
        (
            "distill",
            "Distill (archive)",
            "https://distill.pub/rss.xml",
            "research",
            0.95,
            10080,
        ),
        (
            "papers-with-code",
            "Papers With Code",
            "https://paperswithcode.com/feed.atom",
            "research",
            0.85,
            720,
        ),
        (
            "vercel-blog",
            "Vercel Blog",
            "https://vercel.com/atom",
            "devtools",
            0.7,
            720,
        ),
        (
            "supabase-blog",
            "Supabase Blog",
            "https://supabase.com/feed.xml",
            "devtools",
            0.7,
            720,
        ),
        (
            "cloudflare-blog",
            "Cloudflare Blog",
            "https://blog.cloudflare.com/rss/",
            "devtools",
            0.7,
            720,
        ),
        (
            "github-changelog",
            "GitHub Changelog",
            "https://github.blog/changelog/feed/",
            "devtools",
            0.8,
            720,
        ),
        (
            "stratechery",
            "Stratechery",
            "https://stratechery.com/feed/",
            "media",
            0.85,
            1440,
        ),
        (
            "simonw-tils",
            "Simon Willison TIL",
            "https://til.simonwillison.net/tils/feed.atom",
            "research",
            0.85,
            720,
        ),
        (
            "simonw-blog",
            "Simon Willison Weblog",
            "https://simonwillison.net/atom/everything/",
            "research",
            0.9,
            360,
        ),
        (
            "mistral-news",
            "Mistral AI News",
            "https://mistral.ai/feed.xml",
            "official-ai",
            0.85,
            360,
        ),
    ];

    let feeds = entries
        .iter()
        .map(|(id, name, url, cat, trust, interval)| FeedSource {
            id: (*id).to_string(),
            name: (*name).to_string(),
            url: (*url).to_string(),
            kind: FeedKind::Rss,
            category: (*cat).to_string(),
            trust_weight: *trust,
            enabled: true,
            fetch_interval_minutes: *interval,
        })
        .collect();

    FeedConfig {
        feeds,
        window_days: 7,
        relevance_threshold: 0.4,
        project_keywords_source: ProjectKeywordsSource::ImperiumIdentity,
        brief_paths: BriefPaths::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn default_packet_has_30_entries() {
        let cfg = default_feeds();
        assert!(
            cfg.feeds.len() >= 28 && cfg.feeds.len() <= 40,
            "expected ~30 feeds, got {}",
            cfg.feeds.len()
        );
        assert_eq!(cfg.window_days, 7);
        assert!((cfg.relevance_threshold - 0.4).abs() < 1e-6);
    }

    #[test]
    fn default_packet_ids_unique() {
        let cfg = default_feeds();
        let mut ids: Vec<_> = cfg.feeds.iter().map(|f| f.id.clone()).collect();
        ids.sort();
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n, "duplicate feed ids in default packet");
    }

    #[test]
    fn save_and_load_round_trip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("feeds.yaml");
        let cfg = default_feeds();
        cfg.save(&path).unwrap();
        assert!(path.exists());
        let loaded = FeedConfig::load(&path).unwrap();
        assert_eq!(loaded.feeds.len(), cfg.feeds.len());
        assert_eq!(loaded.window_days, cfg.window_days);
    }

    #[test]
    fn add_and_remove_feeds() {
        let mut cfg = default_feeds();
        let n0 = cfg.feeds.len();
        cfg.add(FeedSource {
            id: "test-new".into(),
            name: "Test".into(),
            url: "https://example.com/rss".into(),
            kind: FeedKind::Rss,
            category: "test".into(),
            trust_weight: 0.5,
            enabled: true,
            fetch_interval_minutes: 60,
        })
        .unwrap();
        assert_eq!(cfg.feeds.len(), n0 + 1);
        assert!(cfg.find("test-new").is_some());

        let removed = cfg.remove("test-new");
        assert!(removed);
        assert_eq!(cfg.feeds.len(), n0);
    }

    #[test]
    fn add_duplicate_id_fails() {
        let mut cfg = default_feeds();
        let dup = cfg.feeds[0].clone();
        let res = cfg.add(dup);
        assert!(res.is_err());
    }

    #[test]
    fn enabled_iter_filters() {
        let mut cfg = default_feeds();
        cfg.feeds[0].enabled = false;
        let n_total = cfg.feeds.len();
        let n_enabled = cfg.enabled().count();
        assert_eq!(n_enabled, n_total - 1);
    }
}

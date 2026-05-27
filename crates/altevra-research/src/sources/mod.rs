//! Unified source provider trait — all research providers (RSS, GitHub Trending,
//! Web Search, Monitor Page) implement this. Brain orchestrates them through a
//! single dispatch path.

use crate::fetcher::FeedItem;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub mod github_trending;
pub mod rss;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    Rss,
    GitHubTrending,
    WebSearch,
    MonitorPage,
}

impl SourceKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Rss => "rss",
            Self::GitHubTrending => "github-trending",
            Self::WebSearch => "web-search",
            Self::MonitorPage => "monitor-page",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "rss" => Self::Rss,
            "github-trending" => Self::GitHubTrending,
            "web-search" => Self::WebSearch,
            "monitor-page" => Self::MonitorPage,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct FetchCtx {
    pub window_days: u32,
    pub query: Option<String>,
    pub limit: usize,
    pub language: Option<String>,
}

#[async_trait]
pub trait SourceProvider: Send + Sync {
    fn id(&self) -> &str;
    fn kind(&self) -> SourceKind;
    async fn fetch(&self, ctx: &FetchCtx) -> anyhow::Result<Vec<FeedItem>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_kind_roundtrip() {
        for k in [
            SourceKind::Rss,
            SourceKind::GitHubTrending,
            SourceKind::WebSearch,
            SourceKind::MonitorPage,
        ] {
            assert_eq!(SourceKind::from_str(k.as_str()), Some(k));
        }
        assert!(SourceKind::from_str("nope").is_none());
    }
}

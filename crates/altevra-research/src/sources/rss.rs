//! RSS/Atom source — wraps `fetch_feed` in the unified SourceProvider trait.

use super::{FetchCtx, SourceKind, SourceProvider};
use crate::feeds::FeedSource;
use crate::fetcher::{fetch_feed, FeedItem, FetchCacheHints};
use async_trait::async_trait;

pub struct RssSource {
    pub feed: FeedSource,
    pub cache: FetchCacheHints,
}

impl RssSource {
    pub fn new(feed: FeedSource) -> Self {
        Self {
            feed,
            cache: FetchCacheHints::default(),
        }
    }
}

#[async_trait]
impl SourceProvider for RssSource {
    fn id(&self) -> &str {
        &self.feed.id
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Rss
    }

    async fn fetch(&self, ctx: &FetchCtx) -> anyhow::Result<Vec<FeedItem>> {
        let outcome = fetch_feed(&self.feed, ctx.window_days, &self.cache).await?;
        Ok(outcome.items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feeds::FeedKind;

    #[test]
    fn rss_source_kind() {
        let f = FeedSource {
            id: "test".into(),
            name: "T".into(),
            url: "https://example.invalid/feed".into(),
            kind: FeedKind::Rss,
            category: "test".into(),
            trust_weight: 0.5,
            enabled: true,
            fetch_interval_minutes: 60,
        };
        let s = RssSource::new(f);
        assert_eq!(s.kind(), SourceKind::Rss);
        assert_eq!(s.id(), "test");
    }
}

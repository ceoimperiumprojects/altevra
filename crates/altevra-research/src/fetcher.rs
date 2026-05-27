//! Feed fetcher — pulls RSS/Atom/JSONFeed sources via reqwest and parses with
//! feed-rs. Supports HTTP cache headers (ETag, Last-Modified) and time-window
//! filtering.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::feeds::FeedSource;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedItem {
    pub feed_id: String,
    pub guid: String,
    pub link: String,
    pub title: String,
    pub summary: String,
    pub published_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default)]
pub struct FetchCacheHints {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FetchOutcome {
    pub items: Vec<FeedItem>,
    pub new_etag: Option<String>,
    pub new_last_modified: Option<String>,
    /// HTTP status code; 304 means "not modified" and `items` will be empty.
    pub status: u16,
}

/// Fetch and parse a feed source, returning items inside `window_days`.
pub async fn fetch_feed(
    source: &FeedSource,
    window_days: u32,
    cache: &FetchCacheHints,
) -> anyhow::Result<FetchOutcome> {
    let client = reqwest::Client::builder()
        .user_agent(
            "Altevra/0.3 research-fetcher (+https://github.com/ceoimperiumprojects/altevra)",
        )
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let mut req = client.get(&source.url);
    if let Some(e) = &cache.etag {
        req = req.header("If-None-Match", e);
    }
    if let Some(lm) = &cache.last_modified {
        req = req.header("If-Modified-Since", lm);
    }

    let resp = req.send().await?;
    let status = resp.status().as_u16();
    let new_etag = resp
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let new_last_modified = resp
        .headers()
        .get("last-modified")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    if status == 304 {
        return Ok(FetchOutcome {
            items: Vec::new(),
            new_etag,
            new_last_modified,
            status,
        });
    }

    let body = resp.bytes().await?;
    let parsed = feed_rs::parser::parse(&body[..])?;
    let cutoff = Utc::now() - Duration::days(window_days as i64);

    let mut items = Vec::new();
    let mut seen = HashSet::new();
    for entry in parsed.entries {
        let link = entry
            .links
            .iter()
            .find(|l| l.rel.as_deref() == Some("alternate") || l.rel.is_none())
            .map(|l| l.href.clone())
            .unwrap_or_else(|| entry.id.clone());

        let guid = if !entry.id.is_empty() {
            entry.id.clone()
        } else {
            link.clone()
        };

        if !seen.insert(guid.clone()) {
            continue;
        }

        let title = entry.title.map(|t| t.content).unwrap_or_default();
        let summary = entry
            .summary
            .map(|s| s.content)
            .or_else(|| entry.content.and_then(|c| c.body))
            .unwrap_or_default();

        let published_at = entry.published.or(entry.updated);

        if let Some(p) = published_at {
            if p < cutoff {
                continue;
            }
        }

        items.push(FeedItem {
            feed_id: source.id.clone(),
            guid,
            link,
            title: clean_text(&title),
            summary: clean_text(&summary),
            published_at,
        });
    }

    Ok(FetchOutcome {
        items,
        new_etag,
        new_last_modified,
        status,
    })
}

/// Strip HTML tags + collapse whitespace. Tiny readability helper, not a full parser.
fn clean_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        if c == '<' {
            in_tag = true;
        } else if c == '>' {
            in_tag = false;
        } else if !in_tag {
            out.push(c);
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feeds::{FeedKind, FeedSource};

    fn dummy_source() -> FeedSource {
        FeedSource {
            id: "test".into(),
            name: "Test".into(),
            url: "https://example.com/rss".into(),
            kind: FeedKind::Rss,
            category: "test".into(),
            trust_weight: 0.5,
            enabled: true,
            fetch_interval_minutes: 60,
        }
    }

    #[test]
    fn clean_text_strips_html() {
        assert_eq!(clean_text("<p>Hello <b>world</b></p>"), "Hello world");
        assert_eq!(clean_text("  spaced \n out  "), "spaced out");
    }

    #[tokio::test]
    async fn parse_rss_filters_by_window() {
        // Generate an RSS doc with a recent and an old entry.
        let recent = Utc::now() - Duration::days(1);
        let old = Utc::now() - Duration::days(60);
        let body = format!(
            r#"<?xml version="1.0"?>
            <rss version="2.0"><channel>
              <title>Test</title>
              <item>
                <guid isPermaLink="false">guid-recent</guid>
                <link>https://example.com/recent</link>
                <title>Recent post</title>
                <description>Recent body</description>
                <pubDate>{}</pubDate>
              </item>
              <item>
                <guid isPermaLink="false">guid-old</guid>
                <link>https://example.com/old</link>
                <title>Old post</title>
                <description>Old body</description>
                <pubDate>{}</pubDate>
              </item>
            </channel></rss>"#,
            recent.to_rfc2822(),
            old.to_rfc2822()
        );

        let parsed = feed_rs::parser::parse(body.as_bytes()).unwrap();
        let cutoff = Utc::now() - Duration::days(7);
        let kept: Vec<_> = parsed
            .entries
            .iter()
            .filter(|e| {
                e.published
                    .or(e.updated)
                    .map(|p| p >= cutoff)
                    .unwrap_or(true)
            })
            .collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id, "guid-recent");
    }

    #[test]
    fn fetch_outcome_dedupes_by_guid() {
        // Verify our dedup logic over a synthetic items list — simulates the
        // post-parse loop in `fetch_feed`.
        let mut seen = HashSet::new();
        let raw = vec!["a", "b", "a", "c", "b"];
        let kept: Vec<_> = raw
            .into_iter()
            .filter(|g| seen.insert(g.to_string()))
            .collect();
        assert_eq!(kept, vec!["a", "b", "c"]);
    }

    #[tokio::test]
    async fn fetch_handles_dns_failure_gracefully() {
        let mut s = dummy_source();
        s.url = "https://this-host-does-not-exist-altevra-test.invalid/feed".into();
        let res = fetch_feed(&s, 7, &FetchCacheHints::default()).await;
        // We expect an error, not a panic.
        assert!(res.is_err());
    }
}

//! GitHub Trending source — scrapes https://github.com/trending/<lang>?since=<period>
//! HTML and yields synthetic FeedItem rows (one per trending repo).
//!
//! GitHub does not expose trending via API or RSS, so this is plain HTML scrape.
//! Parser is defensive: skips entries that don't have a recognizable shape so
//! a future GitHub HTML change degrades to "empty result" instead of panic.

use super::{FetchCtx, SourceKind, SourceProvider};
use crate::fetcher::FeedItem;
use async_trait::async_trait;
use chrono::Utc;
use scraper::{Html, Selector};

#[derive(Debug, Clone)]
pub struct GitHubTrendingSource {
    pub language: Option<String>, // None = all languages
    pub since: TrendingPeriod,
}

#[derive(Debug, Clone, Copy)]
pub enum TrendingPeriod {
    Daily,
    Weekly,
    Monthly,
}

impl TrendingPeriod {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
        }
    }
}

impl GitHubTrendingSource {
    pub fn new(language: Option<String>, since: TrendingPeriod) -> Self {
        Self { language, since }
    }

    pub fn url(&self) -> String {
        let lang = self.language.as_deref().unwrap_or("");
        if lang.is_empty() {
            format!("https://github.com/trending?since={}", self.since.as_str())
        } else {
            format!(
                "https://github.com/trending/{}?since={}",
                lang,
                self.since.as_str()
            )
        }
    }

    pub fn id_str(&self) -> String {
        let lang = self.language.as_deref().unwrap_or("all");
        format!("github-trending-{lang}-{}", self.since.as_str())
    }
}

/// Parse GitHub trending HTML and return FeedItem entries. Pulls repo path
/// (e.g. "rust-lang/rust"), description text, and constructs absolute link.
pub fn parse_trending_html(feed_id: &str, html: &str) -> Vec<FeedItem> {
    let doc = Html::parse_document(html);
    let article_sel = match Selector::parse("article.Box-row") {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let h2_sel = Selector::parse("h2 a").unwrap();
    let desc_sel = Selector::parse("p").unwrap();

    let mut out = Vec::new();
    for article in doc.select(&article_sel) {
        let Some(a) = article.select(&h2_sel).next() else {
            continue;
        };
        let href = a.value().attr("href").unwrap_or("");
        if href.is_empty() {
            continue;
        }
        // GitHub returns hrefs like "/owner/repo" — normalize.
        let path = href.trim().trim_start_matches('/');
        let link = format!("https://github.com/{}", path);
        let title = path.to_string();
        let description = article
            .select(&desc_sel)
            .next()
            .map(|p| p.text().collect::<Vec<_>>().join(" "))
            .unwrap_or_default()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");

        out.push(FeedItem {
            feed_id: feed_id.to_string(),
            guid: link.clone(),
            link,
            title,
            summary: description,
            published_at: Some(Utc::now()),
        });
    }
    out
}

#[async_trait]
impl SourceProvider for GitHubTrendingSource {
    fn id(&self) -> &str {
        // We can't return owned String, so callers use id_str() when they need it.
        // Trait id() is informational only.
        "github-trending"
    }

    fn kind(&self) -> SourceKind {
        SourceKind::GitHubTrending
    }

    async fn fetch(&self, _ctx: &FetchCtx) -> anyhow::Result<Vec<FeedItem>> {
        let client = reqwest::Client::builder()
            .user_agent(
                "Altevra/0.3 research-fetcher (+https://github.com/ceoimperiumprojects/altevra)",
            )
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        let url = self.url();
        let resp = client.get(&url).send().await?;
        let status = resp.status().as_u16();
        if status >= 400 {
            anyhow::bail!("github trending returned status {status}");
        }
        let html = resp.text().await?;
        Ok(parse_trending_html(&self.id_str(), &html))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_with_language() {
        let s = GitHubTrendingSource::new(Some("rust".into()), TrendingPeriod::Daily);
        assert_eq!(s.url(), "https://github.com/trending/rust?since=daily");
        assert_eq!(s.id_str(), "github-trending-rust-daily");
    }

    #[test]
    fn url_without_language() {
        let s = GitHubTrendingSource::new(None, TrendingPeriod::Weekly);
        assert_eq!(s.url(), "https://github.com/trending?since=weekly");
        assert_eq!(s.id_str(), "github-trending-all-weekly");
    }

    #[test]
    fn parse_trending_html_extracts_repos() {
        // Minimal HTML shaped like GitHub's trending page structure.
        let html = r#"
        <html><body>
          <article class="Box-row">
            <h2><a href="/rust-lang/rust">rust-lang/rust</a></h2>
            <p>The Rust programming language</p>
          </article>
          <article class="Box-row">
            <h2><a href="/owner/repo2">owner/repo2</a></h2>
            <p>Another cool project</p>
          </article>
        </body></html>"#;
        let items = parse_trending_html("test-feed", html);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "rust-lang/rust");
        assert_eq!(items[0].link, "https://github.com/rust-lang/rust");
        assert!(items[0].summary.contains("Rust programming"));
        assert_eq!(items[1].title, "owner/repo2");
    }

    #[test]
    fn parse_trending_html_handles_empty() {
        let items = parse_trending_html("test-feed", "<html></html>");
        assert!(items.is_empty());
    }

    #[test]
    fn parse_trending_html_skips_entries_without_href() {
        let html = r#"<html><body>
          <article class="Box-row"><h2><a>no href</a></h2></article>
          <article class="Box-row"><h2><a href="/ok/repo">ok/repo</a></h2></article>
        </body></html>"#;
        let items = parse_trending_html("t", html);
        assert_eq!(items.len(), 1);
    }
}

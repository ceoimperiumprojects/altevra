//! Web search source — pluggable across DuckDuckGo HTML (free, no key),
//! Brave Search API (opt-in via BRAVE_API_KEY), and Exa Search (opt-in via
//! EXA_API_KEY). Tries the configured chain in order; first non-empty wins.
//!
//! DDG HTML is "free but fragile" — they can change markup. We parse defensively;
//! if the layout shifts we return an empty list instead of panicking.

use super::{FetchCtx, SourceKind, SourceProvider};
use crate::fetcher::FeedItem;
use async_trait::async_trait;
use chrono::Utc;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WebSearchProviderKind {
    DuckDuckGo,
    Brave,
    Exa,
}

#[derive(Debug, Clone)]
pub struct WebSearchSource {
    pub query: String,
    pub providers: Vec<WebSearchProviderKind>,
    /// Optional API keys (Brave/Exa). Loaded from keyring or env by callers.
    pub brave_key: Option<String>,
    pub exa_key: Option<String>,
}

impl WebSearchSource {
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            providers: vec![WebSearchProviderKind::DuckDuckGo],
            brave_key: None,
            exa_key: None,
        }
    }

    pub fn with_chain(mut self, providers: Vec<WebSearchProviderKind>) -> Self {
        self.providers = providers;
        self
    }

    pub fn with_brave(mut self, key: impl Into<String>) -> Self {
        self.brave_key = Some(key.into());
        self
    }

    pub fn with_exa(mut self, key: impl Into<String>) -> Self {
        self.exa_key = Some(key.into());
        self
    }
}

#[async_trait]
impl SourceProvider for WebSearchSource {
    fn id(&self) -> &str {
        "web-search"
    }

    fn kind(&self) -> SourceKind {
        SourceKind::WebSearch
    }

    async fn fetch(&self, ctx: &FetchCtx) -> anyhow::Result<Vec<FeedItem>> {
        let limit = ctx.limit.max(1);
        for provider in &self.providers {
            let res = match provider {
                WebSearchProviderKind::DuckDuckGo => search_duckduckgo(&self.query, limit).await,
                WebSearchProviderKind::Brave => match &self.brave_key {
                    Some(k) => search_brave(&self.query, k, limit).await,
                    None => Ok(Vec::new()),
                },
                WebSearchProviderKind::Exa => match &self.exa_key {
                    Some(k) => search_exa(&self.query, k, limit).await,
                    None => Ok(Vec::new()),
                },
            };
            match res {
                Ok(items) if !items.is_empty() => return Ok(items),
                Ok(_) => continue,
                Err(e) => {
                    tracing::warn!("web search provider {:?} failed: {e}", provider);
                    continue;
                }
            }
        }
        Ok(Vec::new())
    }
}

/// DuckDuckGo lite search — POST to lite.duckduckgo.com/lite/ with the query
/// in form data. Lite endpoint is more scraper-friendly than the html/ one
/// (which triggers an anomaly modal in 2026). Parser is defensive.
///
/// Note: even the lite endpoint can sporadically block. If you need reliable
/// web search at scale, set BRAVE_API_KEY (free 2000/mo) or EXA_API_KEY.
pub async fn search_duckduckgo(query: &str, limit: usize) -> anyhow::Result<Vec<FeedItem>> {
    let client = reqwest::Client::builder()
        .user_agent(
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0",
        )
        .timeout(std::time::Duration::from_secs(15))
        .build()?;
    let form = [("q", query), ("kl", "us-en")];
    let resp = client
        .post("https://lite.duckduckgo.com/lite/")
        .form(&form)
        .header("Accept", "text/html,application/xhtml+xml")
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await?;
    let html = resp.text().await?;
    Ok(parse_ddg_lite(&html, query, limit))
}

/// Parse the DDG /lite/ endpoint HTML (simpler than /html/). Each result is a
/// `<a class="result-link">` followed by a `<td class="result-snippet">`.
pub fn parse_ddg_lite(html: &str, query: &str, limit: usize) -> Vec<FeedItem> {
    let doc = Html::parse_document(html);
    let link_sel = match Selector::parse("a.result-link") {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let snippet_sel = Selector::parse("td.result-snippet").unwrap();

    let mut items = Vec::new();
    let links: Vec<_> = doc.select(&link_sel).take(limit).collect();
    let snippets: Vec<_> = doc.select(&snippet_sel).collect();
    for (i, a) in links.iter().enumerate() {
        let raw_href = a.value().attr("href").unwrap_or("");
        let link = unwrap_ddg_redirect(raw_href);
        if link.is_empty() {
            continue;
        }
        let title = a.text().collect::<Vec<_>>().join(" ").trim().to_string();
        let summary = snippets
            .get(i)
            .map(|s| {
                s.text()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        items.push(FeedItem {
            feed_id: format!("web-search:{}", query),
            guid: link.clone(),
            link,
            title,
            summary,
            published_at: Some(Utc::now()),
        });
    }
    items
}

pub fn parse_ddg_html(html: &str, query: &str, limit: usize) -> Vec<FeedItem> {
    let doc = Html::parse_document(html);
    // DDG html version uses `.result` rows; each has `.result__a` (link) and
    // `.result__snippet` (body). Skip ad rows (`.result--ad`).
    let row_sel = match Selector::parse(".result:not(.result--ad)") {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let link_sel = Selector::parse("a.result__a").unwrap();
    let snippet_sel = Selector::parse(".result__snippet").unwrap();

    let mut items = Vec::new();
    for row in doc.select(&row_sel).take(limit) {
        let Some(a) = row.select(&link_sel).next() else {
            continue;
        };
        let raw_href = a.value().attr("href").unwrap_or("");
        // DDG wraps real URLs behind /l/?uddg=<encoded>; if present, decode.
        let link = unwrap_ddg_redirect(raw_href);
        if link.is_empty() {
            continue;
        }
        let title = a.text().collect::<Vec<_>>().join(" ").trim().to_string();
        let summary = row
            .select(&snippet_sel)
            .next()
            .map(|s| s.text().collect::<Vec<_>>().join(" "))
            .unwrap_or_default()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        items.push(FeedItem {
            feed_id: format!("web-search:{}", query),
            guid: link.clone(),
            link,
            title,
            summary,
            published_at: Some(Utc::now()),
        });
    }
    items
}

fn unwrap_ddg_redirect(href: &str) -> String {
    if let Some(idx) = href.find("uddg=") {
        let encoded = &href[idx + 5..];
        let end = encoded.find('&').unwrap_or(encoded.len());
        let segment = &encoded[..end];
        return percent_decode(segment);
    }
    if href.starts_with("//") {
        return format!("https:{href}");
    }
    href.to_string()
}

fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) =
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
            {
                out.push(byte as char);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else if b == b' ' {
            out.push('+');
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

/// Brave Search API — requires X-Subscription-Token header.
pub async fn search_brave(
    query: &str,
    api_key: &str,
    limit: usize,
) -> anyhow::Result<Vec<FeedItem>> {
    let client = reqwest::Client::builder()
        .user_agent("Altevra/0.3 brave-search")
        .timeout(std::time::Duration::from_secs(15))
        .build()?;
    let url = format!(
        "https://api.search.brave.com/res/v1/web/search?q={}&count={}",
        urlencoding(query),
        limit
    );
    let resp = client
        .get(&url)
        .header("X-Subscription-Token", api_key)
        .send()
        .await?;
    let body: serde_json::Value = resp.json().await?;
    let results = body
        .get("web")
        .and_then(|w| w.get("results"))
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();
    let mut items = Vec::new();
    for r in results.into_iter().take(limit) {
        let title = r
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let url_v = r
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        if url_v.is_empty() {
            continue;
        }
        let desc = r
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        items.push(FeedItem {
            feed_id: format!("web-search-brave:{query}"),
            guid: url_v.clone(),
            link: url_v,
            title,
            summary: desc,
            published_at: Some(Utc::now()),
        });
    }
    Ok(items)
}

/// Exa Search (formerly Metaphor) — Bearer token in Authorization header.
pub async fn search_exa(query: &str, api_key: &str, limit: usize) -> anyhow::Result<Vec<FeedItem>> {
    let client = reqwest::Client::builder()
        .user_agent("Altevra/0.3 exa-search")
        .timeout(std::time::Duration::from_secs(15))
        .build()?;
    let body = serde_json::json!({
        "query": query,
        "num_results": limit,
        "use_autoprompt": true,
    });
    let resp = client
        .post("https://api.exa.ai/search")
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await?;
    let parsed: serde_json::Value = resp.json().await?;
    let results = parsed
        .get("results")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();
    let mut items = Vec::new();
    for r in results.into_iter().take(limit) {
        let title = r.get("title").and_then(|v| v.as_str()).unwrap_or_default();
        let url_v = r.get("url").and_then(|v| v.as_str()).unwrap_or_default();
        if url_v.is_empty() {
            continue;
        }
        items.push(FeedItem {
            feed_id: format!("web-search-exa:{query}"),
            guid: url_v.to_string(),
            link: url_v.to_string(),
            title: title.to_string(),
            summary: String::new(),
            published_at: Some(Utc::now()),
        });
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlencoding_works() {
        assert_eq!(urlencoding("hello world"), "hello+world");
        assert_eq!(urlencoding("a&b"), "a%26b");
        assert_eq!(urlencoding("rust"), "rust");
    }

    #[test]
    fn percent_decode_works() {
        assert_eq!(percent_decode("hello%20world"), "hello world");
        assert_eq!(percent_decode("a%26b"), "a&b");
    }

    #[test]
    fn ddg_redirect_unwraps_uddg_param() {
        let u = "/l/?uddg=https%3A%2F%2Fexample.com%2Fpost&rut=foo";
        assert_eq!(unwrap_ddg_redirect(u), "https://example.com/post");
    }

    #[test]
    fn ddg_redirect_handles_protocol_relative() {
        assert_eq!(
            unwrap_ddg_redirect("//example.com/x"),
            "https://example.com/x"
        );
    }

    #[test]
    fn ddg_redirect_passthrough_for_normal_urls() {
        assert_eq!(
            unwrap_ddg_redirect("https://example.com/y"),
            "https://example.com/y"
        );
    }

    #[test]
    fn parse_ddg_html_extracts_results() {
        let html = r##"
        <div class="result">
          <a class="result__a" href="https://example.com/a">Example A</a>
          <span class="result__snippet">Snippet A</span>
        </div>
        <div class="result result--ad">
          <a class="result__a" href="/sponsored">Ad — skip</a>
        </div>
        <div class="result">
          <a class="result__a" href="/l/?uddg=https%3A%2F%2Fother.com%2Fb">Other B</a>
          <span class="result__snippet">Snippet B</span>
        </div>"##;
        let items = parse_ddg_html(html, "query", 10);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].link, "https://example.com/a");
        assert_eq!(items[1].link, "https://other.com/b");
        assert!(items[0].title.contains("Example A"));
    }

    #[test]
    fn parse_ddg_html_handles_empty() {
        let items = parse_ddg_html("<html></html>", "q", 5);
        assert!(items.is_empty());
    }

    #[test]
    fn parse_ddg_html_respects_limit() {
        let mut html = String::new();
        for i in 0..10 {
            html.push_str(&format!(
                r#"<div class="result"><a class="result__a" href="https://e{i}.com/">T{i}</a></div>"#
            ));
        }
        let items = parse_ddg_html(&html, "q", 3);
        assert_eq!(items.len(), 3);
    }

    #[tokio::test]
    async fn web_search_falls_back_when_brave_key_missing() {
        // Brave without key should yield empty; DDG fallback may yield items
        // (or empty if offline) — but no panic.
        let s = WebSearchSource::new("rust").with_chain(vec![
            WebSearchProviderKind::Brave,
            WebSearchProviderKind::DuckDuckGo,
        ]);
        let ctx = FetchCtx {
            limit: 5,
            ..Default::default()
        };
        let _ = s.fetch(&ctx).await; // no assertion — network-dependent
    }
}

//! Feed discovery — find RSS/Atom/JSON-Feed URLs from a generic web page.
//!
//! Strategy:
//! 1. Parse `<link rel="alternate" type="application/rss+xml">` (and atom/json variants)
//! 2. Try canonical paths: /feed, /rss, /atom.xml, /index.xml
//! 3. Optionally walk outbound links for blog/news candidates
//! 4. Read /robots.txt → sitemap URL

use scraper::{Html, Selector};
use std::collections::HashSet;
use url::Url;

const FEED_MIME_TYPES: &[&str] = &[
    "application/rss+xml",
    "application/atom+xml",
    "application/json",
    "application/feed+json",
];

const CANONICAL_FEED_PATHS: &[&str] = &[
    "/feed",
    "/feed/",
    "/rss",
    "/rss.xml",
    "/atom.xml",
    "/index.xml",
    "/feed.xml",
];

/// Extract candidate feed URLs from an HTML page.
/// Returns absolute URLs. Filters duplicates.
pub fn extract_feed_links(base_url: &str, html: &str) -> Vec<String> {
    let mut found: HashSet<String> = HashSet::new();
    let base = Url::parse(base_url).ok();

    let doc = Html::parse_document(html);
    let link_sel = match Selector::parse("link[rel='alternate']") {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };

    for link in doc.select(&link_sel) {
        let ty = link.value().attr("type").unwrap_or("");
        if !FEED_MIME_TYPES.contains(&ty) {
            continue;
        }
        let href = link.value().attr("href").unwrap_or("");
        if href.is_empty() {
            continue;
        }
        let abs = resolve_url(base.as_ref(), href);
        if !abs.is_empty() {
            found.insert(abs);
        }
    }

    // Always try canonical paths too (cheap, deduped at HEAD probe time).
    if let Some(b) = &base {
        if let Ok(origin) = Url::parse(&format!(
            "{}://{}",
            b.scheme(),
            b.host_str().unwrap_or_default()
        )) {
            for p in CANONICAL_FEED_PATHS {
                if let Ok(u) = origin.join(p) {
                    found.insert(u.to_string());
                }
            }
        }
    }

    let mut out: Vec<String> = found.into_iter().collect();
    out.sort();
    out
}

/// All outbound `<a href>` links on a page, absolutized. De-duplicated.
pub fn extract_outbound_links(base_url: &str, html: &str) -> Vec<String> {
    let mut found: HashSet<String> = HashSet::new();
    let base = Url::parse(base_url).ok();

    let doc = Html::parse_document(html);
    let a_sel = match Selector::parse("a[href]") {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    for a in doc.select(&a_sel) {
        let href = a.value().attr("href").unwrap_or("");
        if href.is_empty() || href.starts_with('#') || href.starts_with("javascript:") {
            continue;
        }
        let abs = resolve_url(base.as_ref(), href);
        if !abs.is_empty() && (abs.starts_with("http://") || abs.starts_with("https://")) {
            found.insert(abs);
        }
    }
    let mut out: Vec<String> = found.into_iter().collect();
    out.sort();
    out
}

/// Heuristically filter outbound links that look like blog/news/article posts
/// (good candidates for "this domain probably has an RSS feed").
pub fn filter_promising_blog_links(links: &[String]) -> Vec<String> {
    let needles = ["/blog", "/news", "/post", "/article", "/p/", "/feed"];
    links
        .iter()
        .filter(|l| {
            let lc = l.to_lowercase();
            // Skip static asset / CDN paths.
            !lc.contains("/cdn-cgi/")
                && !lc.contains("/static/")
                && !lc.contains(".css")
                && !lc.contains(".js")
                && !lc.contains(".png")
                && !lc.contains(".jpg")
                && needles.iter().any(|n| lc.contains(n))
        })
        .cloned()
        .collect()
}

/// Extract Sitemap URL from a robots.txt body. Returns first match, if any.
pub fn extract_sitemap_url(robots_txt: &str) -> Option<String> {
    for line in robots_txt.lines() {
        let line = line.trim();
        if let Some(url) = line
            .strip_prefix("Sitemap:")
            .or_else(|| line.strip_prefix("sitemap:"))
        {
            let url = url.trim();
            if !url.is_empty() {
                return Some(url.to_string());
            }
        }
    }
    None
}

fn resolve_url(base: Option<&Url>, href: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") {
        return href.to_string();
    }
    let Some(b) = base else { return String::new() };
    b.join(href).map(|u| u.to_string()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_rss_link_tag() {
        let html = r#"
        <html><head>
          <link rel="alternate" type="application/rss+xml" href="/feed.xml" title="Site RSS">
        </head></html>"#;
        let feeds = extract_feed_links("https://example.com/blog", html);
        assert!(
            feeds.iter().any(|f| f == "https://example.com/feed.xml"),
            "expected feed.xml in {feeds:?}"
        );
    }

    #[test]
    fn extracts_atom_link_tag() {
        let html = r#"
        <link rel="alternate" type="application/atom+xml" href="https://other.com/feed">"#;
        let feeds = extract_feed_links("https://example.com", html);
        assert!(feeds.iter().any(|f| f == "https://other.com/feed"));
    }

    #[test]
    fn extracts_json_feed_link_tag() {
        let html = r#"<link rel="alternate" type="application/feed+json" href="/feed.json">"#;
        let feeds = extract_feed_links("https://example.com", html);
        assert!(feeds.iter().any(|f| f.ends_with("/feed.json")));
    }

    #[test]
    fn extract_feed_links_always_includes_canonicals() {
        let feeds = extract_feed_links("https://example.com/", "<html></html>");
        assert!(feeds.iter().any(|f| f == "https://example.com/feed"));
        assert!(feeds.iter().any(|f| f == "https://example.com/rss"));
    }

    #[test]
    fn outbound_links_absolutized_and_deduped() {
        let html = r##"
        <a href="/page1">P1</a>
        <a href="/page1">P1 again</a>
        <a href="https://other.com/x">other</a>
        <a href="#anchor">skip</a>
        <a href="javascript:void(0)">skip2</a>"##;
        let links = extract_outbound_links("https://example.com", html);
        assert!(links.iter().any(|l| l == "https://example.com/page1"));
        assert!(links.iter().any(|l| l == "https://other.com/x"));
        assert!(!links.iter().any(|l| l.contains("anchor")));
        // Deduped: page1 appears once.
        assert_eq!(links.iter().filter(|l| l.contains("/page1")).count(), 1);
    }

    #[test]
    fn promising_blog_links_filters() {
        let raw = vec![
            "https://example.com/blog/post1".to_string(),
            "https://example.com/static/main.css".to_string(),
            "https://example.com/news/today".to_string(),
            "https://example.com/cdn-cgi/foo".to_string(),
            "https://example.com/about".to_string(),
        ];
        let kept = filter_promising_blog_links(&raw);
        assert!(kept.iter().any(|l| l.contains("/blog/")));
        assert!(kept.iter().any(|l| l.contains("/news/")));
        assert!(!kept.iter().any(|l| l.contains("/static/")));
        assert!(!kept.iter().any(|l| l.contains("/cdn-cgi/")));
        assert!(!kept.iter().any(|l| l.ends_with("/about")));
    }

    #[test]
    fn sitemap_extraction() {
        let robots = "User-agent: *\nAllow: /\nSitemap: https://example.com/sitemap.xml\n";
        assert_eq!(
            extract_sitemap_url(robots).as_deref(),
            Some("https://example.com/sitemap.xml")
        );
        assert!(extract_sitemap_url("User-agent: *\nDisallow: /admin").is_none());
    }
}

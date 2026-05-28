use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScrapedPage {
    pub url: String,
    pub title: Option<String>,
    pub text: String,
    pub html: String,
    pub status: u16,
    pub fetched_at: chrono::DateTime<chrono::Utc>,
}

/// Fetch a URL and extract readable text + title.
pub async fn scrape_url(url: &str) -> anyhow::Result<ScrapedPage> {
    let client = reqwest::Client::builder()
        .user_agent("Altevra/0.1 (+https://github.com/ceoimperiumprojects/altevra)")
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let resp = client.get(url).send().await?;
    let status = resp.status().as_u16();
    let html = resp.text().await?;
    let (title, text) = extract_readable(&html);

    Ok(ScrapedPage {
        url: url.to_string(),
        title,
        text,
        html,
        status,
        fetched_at: chrono::Utc::now(),
    })
}

const STRIP_TAGS: &[&str] = &[
    "script", "style", "nav", "footer", "header", "aside", "noscript",
];

/// Extract title + body text from HTML using a simple readability heuristic.
pub fn extract_readable(html: &str) -> (Option<String>, String) {
    let doc = Html::parse_document(html);
    let title = first_text(&doc, "title").or_else(|| first_text(&doc, "h1"));

    let body_html = ["article", "main", "body"]
        .iter()
        .find_map(|sel| {
            Selector::parse(sel)
                .ok()
                .and_then(|s| doc.select(&s).next().map(|el| el.html()))
        })
        .unwrap_or_default();

    let body_doc = Html::parse_fragment(&body_html);
    let p_selector = Selector::parse("p, li, h1, h2, h3, h4, h5, h6, blockquote").unwrap();

    let mut text = String::new();
    for el in body_doc.select(&p_selector) {
        if has_excluded_ancestor(el) {
            continue;
        }
        let chunk = el
            .text()
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if !chunk.is_empty() {
            text.push_str(&chunk);
            text.push_str("\n\n");
        }
    }
    (title.map(|t| t.trim().to_string()), text.trim().to_string())
}

fn first_text(doc: &Html, selector: &str) -> Option<String> {
    let s = Selector::parse(selector).ok()?;
    doc.select(&s)
        .next()
        .map(|el| el.text().collect::<Vec<_>>().join(" "))
}

fn has_excluded_ancestor(el: scraper::ElementRef<'_>) -> bool {
    let mut current = el.parent();
    while let Some(node) = current {
        if let Some(elem) = node.value().as_element() {
            let tag = elem.name();
            if STRIP_TAGS.contains(&tag) {
                return true;
            }
        }
        current = node.parent();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_title_from_h1() {
        let html = "<html><body><h1>Hello World</h1><p>Body.</p></body></html>";
        let (title, text) = extract_readable(html);
        assert_eq!(title.as_deref(), Some("Hello World"));
        assert!(text.contains("Body."));
    }

    #[test]
    fn extract_title_from_title_tag() {
        let html =
            "<html><head><title>Doc Title</title></head><body><p>Content here.</p></body></html>";
        let (title, _) = extract_readable(html);
        assert_eq!(title.as_deref(), Some("Doc Title"));
    }

    #[test]
    fn skip_scripts() {
        let html = r#"<html><body><script>alert('bad')</script><p>Good text.</p></body></html>"#;
        let (_, text) = extract_readable(html);
        assert!(!text.contains("alert"));
        assert!(text.contains("Good text"));
    }

    #[test]
    fn prefers_article_body() {
        let html = r#"<html><body><nav>skip</nav><article><p>Main content</p></article><footer>also skip</footer></body></html>"#;
        let (_, text) = extract_readable(html);
        assert!(text.contains("Main content"));
    }
}

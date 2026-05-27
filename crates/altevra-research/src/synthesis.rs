use crate::scraper::ScrapedPage;

pub struct SynthesisInput<'a> {
    pub query: &'a str,
    pub pages: &'a [ScrapedPage],
}

/// Local synthesis: concatenates extracted text with provenance, no external LLM.
/// External LLM synthesis is reserved for v0.2+.
pub fn synthesize(input: SynthesisInput<'_>) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Research: {}\n\n", input.query));
    out.push_str(&format!("Sources: {}\n\n", input.pages.len()));

    if input.pages.is_empty() {
        out.push_str("_No sources collected._\n");
        return out;
    }

    out.push_str("## Summary\n\n");
    for page in input.pages {
        let preview: String = page
            .text
            .chars()
            .take(280)
            .collect::<String>()
            .replace('\n', " ");
        let title = page.title.as_deref().unwrap_or("(untitled)");
        out.push_str(&format!("- [{title}]({}) — {preview}\n", page.url));
    }
    out.push_str("\n## Excerpts\n\n");
    for page in input.pages {
        out.push_str(&format!(
            "### {} — {}\n\n{}\n\n---\n\n",
            page.title.as_deref().unwrap_or("(untitled)"),
            page.url,
            page.text.chars().take(2000).collect::<String>(),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn page(url: &str, title: &str, text: &str) -> ScrapedPage {
        ScrapedPage {
            url: url.into(),
            title: Some(title.into()),
            text: text.into(),
            html: String::new(),
            status: 200,
            fetched_at: Utc::now(),
        }
    }

    #[test]
    fn empty_synthesis_has_disclaimer() {
        let out = synthesize(SynthesisInput {
            query: "foo",
            pages: &[],
        });
        assert!(out.contains("No sources collected"));
    }

    #[test]
    fn synthesis_lists_sources() {
        let pages = vec![
            page("https://a.com", "A", "alpha body"),
            page("https://b.com", "B", "beta body"),
        ];
        let out = synthesize(SynthesisInput {
            query: "topic",
            pages: &pages,
        });
        assert!(out.contains("Research: topic"));
        assert!(out.contains("https://a.com"));
        assert!(out.contains("https://b.com"));
        assert!(out.contains("alpha body"));
    }
}

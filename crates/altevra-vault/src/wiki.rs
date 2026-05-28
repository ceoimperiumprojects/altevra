//! Wiki page parsing — the Altevra living-knowledge layer.
//!
//! See `wiki/concepts/wiki-layer.md` for the conceptual definition.
//! See `ALTEVRA_NEXT_ARCHITECTURE_RESIDENT_AGENT_WIKI_PERSONAL_BRAIN.md` §12
//! for the schema spec.
//!
//! A wiki page is a markdown file with typed frontmatter living anywhere
//! under `wiki/`. This module provides a typed view on top of the generic
//! `Frontmatter` parser so callers don't have to reach into YAML by hand.

use chrono::{DateTime, Utc};
use regex::Regex;
use std::path::{Path, PathBuf};

use crate::{parse_document, Frontmatter, ParsedDocument};

/// Lifecycle status of a wiki page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WikiStatus {
    Living,
    Archived,
    Draft,
    Other(String),
}

impl WikiStatus {
    pub fn parse(s: &str) -> Self {
        match s {
            "living" => Self::Living,
            "archived" => Self::Archived,
            "draft" => Self::Draft,
            other => Self::Other(other.to_string()),
        }
    }
    pub fn as_str(&self) -> &str {
        match self {
            Self::Living => "living",
            Self::Archived => "archived",
            Self::Draft => "draft",
            Self::Other(s) => s.as_str(),
        }
    }
}

/// Page-level confidence in the synthesized content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WikiConfidence {
    Low,
    Medium,
    High,
    Other(String),
}

impl WikiConfidence {
    pub fn parse(s: &str) -> Self {
        match s {
            "low" => Self::Low,
            "medium" => Self::Medium,
            "high" => Self::High,
            other => Self::Other(other.to_string()),
        }
    }
    pub fn as_str(&self) -> &str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Other(s) => s.as_str(),
        }
    }
}

/// A typed view on a wiki page.
#[derive(Debug, Clone)]
pub struct WikiPage {
    pub id: String,
    pub topic: String,
    pub status: WikiStatus,
    pub confidence: WikiConfidence,
    pub sensitivity: String,
    pub source_count: u32,
    pub last_synthesized_at: Option<DateTime<Utc>>,
    pub related_projects: Vec<String>,
    pub related_pages: Vec<String>,
    pub owner: String,
    pub title: Option<String>,
    pub body: String,
    pub checksum: String,
    pub path: PathBuf,
    /// `[[topic]]` references extracted from body. Auto-deduplicated, order-preserved.
    pub wiki_links: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum WikiError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("parser: {0}")]
    Parser(String),
    #[error("missing required frontmatter field: {0}")]
    MissingField(String),
}

impl From<anyhow::Error> for WikiError {
    fn from(e: anyhow::Error) -> Self {
        WikiError::Parser(e.to_string())
    }
}

impl WikiPage {
    pub fn from_parsed(path: &Path, parsed: ParsedDocument) -> Result<Self, WikiError> {
        let fm = parsed
            .frontmatter
            .ok_or_else(|| WikiError::MissingField("frontmatter".into()))?;
        let topic = required_str(&fm, "topic")?;
        let id = optional_str(&fm, "id").unwrap_or_else(|| format!("wiki_{topic}"));
        let status = optional_str(&fm, "status")
            .map(|s| WikiStatus::parse(&s))
            .unwrap_or(WikiStatus::Living);
        let confidence = optional_str(&fm, "confidence")
            .map(|s| WikiConfidence::parse(&s))
            .unwrap_or(WikiConfidence::Medium);
        let sensitivity = optional_str(&fm, "sensitivity").unwrap_or_else(|| "internal".into());
        let source_count = optional_u32(&fm, "source_count").unwrap_or(0);
        let last_synthesized_at = optional_str(&fm, "last_synthesized_at").and_then(|s| {
            // Accept both date (YYYY-MM-DD) and full RFC3339.
            DateTime::parse_from_rfc3339(&s)
                .map(|d| d.with_timezone(&Utc))
                .ok()
                .or_else(|| {
                    chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d")
                        .ok()
                        .and_then(|d| d.and_hms_opt(0, 0, 0))
                        .map(|naive| DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc))
                })
        });
        let related_projects = optional_str_array(&fm, "related_projects");
        let related_pages = optional_str_array(&fm, "related_pages");
        let owner = optional_str(&fm, "owner").unwrap_or_else(|| "altevra".into());
        let wiki_links = extract_wiki_links(&parsed.body);

        Ok(Self {
            id,
            topic,
            status,
            confidence,
            sensitivity,
            source_count,
            last_synthesized_at,
            related_projects,
            related_pages,
            owner,
            title: parsed.title,
            body: parsed.body,
            checksum: parsed.checksum,
            path: path.to_path_buf(),
            wiki_links,
        })
    }
}

/// Parse a single wiki page file.
pub fn parse_wiki_page(path: &Path) -> Result<WikiPage, WikiError> {
    let parsed = parse_document(path)?;
    WikiPage::from_parsed(path, parsed)
}

/// Extract `[[topic]]` references from a body. Filters empty/whitespace-only,
/// dedupes by string equality while preserving first-seen order.
pub fn extract_wiki_links(body: &str) -> Vec<String> {
    let re = Regex::new(r"\[\[([^\]\n]+)\]\]").expect("compile-time regex must be valid");
    let mut out = Vec::new();
    for caps in re.captures_iter(body) {
        let raw = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        if raw.is_empty() {
            continue;
        }
        // Strip pipe-aliases: [[topic|display text]] → topic
        let topic = raw.split_once('|').map(|x| x.0.trim()).unwrap_or(raw);
        let s = topic.to_string();
        if !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

/// Walk `root` recursively and parse every `.md` file as a wiki page.
/// Silently skips files that fail to parse (caller can re-run individually to
/// see specific errors).
pub fn list_wiki_pages(root: &Path) -> Result<Vec<WikiPage>, WikiError> {
    if !root.exists() {
        return Ok(vec![]);
    }
    let mut pages = Vec::new();
    for entry in walkdir::WalkDir::new(root)
        .max_depth(5)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let p = entry.path();
        if p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("md") {
            if let Ok(page) = parse_wiki_page(p) {
                pages.push(page);
            }
        }
    }
    // Stable order: by topic.
    pages.sort_by(|a, b| a.topic.cmp(&b.topic));
    Ok(pages)
}

// ─── Frontmatter accessors ──────────────────────────────────────────────────

fn required_str(fm: &Frontmatter, key: &str) -> Result<String, WikiError> {
    optional_str(fm, key).ok_or_else(|| WikiError::MissingField(key.into()))
}

fn optional_str(fm: &Frontmatter, key: &str) -> Option<String> {
    fm.raw
        .as_mapping()
        .and_then(|m| m.get(serde_yaml::Value::String(key.into())))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn optional_u32(fm: &Frontmatter, key: &str) -> Option<u32> {
    fm.raw
        .as_mapping()
        .and_then(|m| m.get(serde_yaml::Value::String(key.into())))
        .and_then(|v| v.as_u64())
        .and_then(|n| u32::try_from(n).ok())
}

fn optional_str_array(fm: &Frontmatter, key: &str) -> Vec<String> {
    fm.raw
        .as_mapping()
        .and_then(|m| m.get(serde_yaml::Value::String(key.into())))
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_md(dir: &Path, name: &str, content: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn extract_wiki_links_basic() {
        let body = "see [[other-topic]] and [[another]] but not regular links";
        let links = extract_wiki_links(body);
        assert_eq!(links, vec!["other-topic", "another"]);
    }

    #[test]
    fn extract_wiki_links_dedupes_and_strips_aliases() {
        let body = "ref [[topic-a]] then [[topic-a|same target]] then [[topic-b]]";
        let links = extract_wiki_links(body);
        assert_eq!(links, vec!["topic-a", "topic-b"]);
    }

    #[test]
    fn parse_minimal_page() {
        let tmp = TempDir::new().unwrap();
        let p = write_md(
            tmp.path(),
            "tiny.md",
            "---\ntopic: tiny\n---\n# Tiny\n\nbody [[other]]\n",
        );
        let page = parse_wiki_page(&p).unwrap();
        assert_eq!(page.topic, "tiny");
        assert_eq!(page.id, "wiki_tiny");
        assert_eq!(page.status, WikiStatus::Living);
        assert_eq!(page.confidence, WikiConfidence::Medium);
        assert_eq!(page.wiki_links, vec!["other"]);
        assert!(page.title.as_deref().unwrap_or("").contains("Tiny"));
    }

    #[test]
    fn parse_full_frontmatter() {
        let tmp = TempDir::new().unwrap();
        let p = write_md(
            tmp.path(),
            "full.md",
            "---\nid: wiki_test\ntopic: test\nstatus: archived\nconfidence: high\nsensitivity: private\nsource_count: 17\nlast_synthesized_at: 2026-05-28\nrelated_projects:\n  - altevra\n  - revesta\nrelated_pages:\n  - other-topic\nowner: altevra\n---\n# Test\n\n[[link-a]] and [[link-b]]\n",
        );
        let page = parse_wiki_page(&p).unwrap();
        assert_eq!(page.id, "wiki_test");
        assert_eq!(page.status, WikiStatus::Archived);
        assert_eq!(page.confidence, WikiConfidence::High);
        assert_eq!(page.sensitivity, "private");
        assert_eq!(page.source_count, 17);
        assert!(page.last_synthesized_at.is_some());
        assert_eq!(page.related_projects, vec!["altevra", "revesta"]);
        assert_eq!(page.related_pages, vec!["other-topic"]);
        assert_eq!(page.wiki_links, vec!["link-a", "link-b"]);
    }

    #[test]
    fn missing_topic_is_required() {
        let tmp = TempDir::new().unwrap();
        let p = write_md(tmp.path(), "bad.md", "---\nid: wiki_bad\n---\n# bad\n");
        let res = parse_wiki_page(&p);
        assert!(matches!(res, Err(WikiError::MissingField(ref f)) if f == "topic"));
    }

    #[test]
    fn list_wiki_pages_walks_subfolders() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("concepts")).unwrap();
        std::fs::create_dir_all(tmp.path().join("projects")).unwrap();
        write_md(
            &tmp.path().join("concepts"),
            "a.md",
            "---\ntopic: alpha\n---\n# Alpha\n",
        );
        write_md(
            &tmp.path().join("projects"),
            "b.md",
            "---\ntopic: beta\n---\n# Beta\n",
        );
        // A non-md file that shouldn't be picked up.
        std::fs::write(tmp.path().join("README.txt"), "ignored").unwrap();
        let pages = list_wiki_pages(tmp.path()).unwrap();
        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0].topic, "alpha");
        assert_eq!(pages[1].topic, "beta");
    }

    #[test]
    fn status_and_confidence_roundtrip() {
        assert_eq!(WikiStatus::parse("living").as_str(), "living");
        assert_eq!(WikiStatus::parse("archived").as_str(), "archived");
        assert_eq!(WikiStatus::parse("draft").as_str(), "draft");
        assert_eq!(WikiStatus::parse("experimental").as_str(), "experimental");
        assert_eq!(WikiConfidence::parse("high").as_str(), "high");
    }
}

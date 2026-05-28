//! Relevance scoring — match feed items against project keywords.
//!
//! Uses a lightweight BM25-inspired token overlap score. When Gemini embedder
//! is available the brain daemon can call into altevra-memory cosine helpers
//! instead, but for the synchronous in-loop path BM25 is enough and works
//! offline.

use crate::fetcher::FeedItem;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ProjectKeywords {
    pub project_id: String,
    pub keywords: Vec<String>,
}

const STOPWORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "but", "if", "of", "in", "on", "at", "to", "for", "with", "by",
    "from", "as", "is", "are", "was", "were", "be", "been", "being", "this", "that", "these",
    "those", "it", "its", "i", "you", "we", "they", "he", "she", "his", "her", "their", "our",
    "your", "my", "have", "has", "had", "do", "does", "did", "will", "would", "could", "should",
    "may", "might", "must", "can", "just", "so", "than", "then", "there", "here", "what", "which",
    "who", "whom", "how", "when", "where", "why", "no", "not", "yes", "all", "any", "some", "more",
    "most", "less", "least", "much", "many", "few", "very", "really", "into", "out", "up", "down",
    "over", "under", "again", "also", "about",
];

fn tokenize(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| !t.is_empty() && !STOPWORDS.contains(t) && t.len() > 2)
        .map(String::from)
        .collect()
}

/// Compute a token-overlap score (Jaccard-ish, weighted by keyword length).
/// Range: 0.0 (no overlap) to ~1.0 (every keyword matched in title/summary).
pub fn score_item(item: &FeedItem, project_keywords: &[String]) -> f32 {
    if project_keywords.is_empty() {
        return 0.0;
    }
    let haystack = format!("{} {}", item.title, item.summary);
    let tokens: HashSet<String> = tokenize(&haystack).into_iter().collect();

    let kw_tokens: HashSet<String> = project_keywords.iter().flat_map(|k| tokenize(k)).collect();

    if kw_tokens.is_empty() {
        return 0.0;
    }

    let matches = kw_tokens.intersection(&tokens).count();
    let denom = kw_tokens.len();
    matches as f32 / denom as f32
}

/// Match an item against many projects, return ids whose score >= threshold.
pub fn matching_projects(
    item: &FeedItem,
    projects: &[ProjectKeywords],
    threshold: f32,
) -> (f32, Vec<String>) {
    let mut max_score = 0.0_f32;
    let mut matched = Vec::new();
    for p in projects {
        let s = score_item(item, &p.keywords);
        if s >= threshold {
            matched.push(p.project_id.clone());
        }
        if s > max_score {
            max_score = s;
        }
    }
    (max_score, matched)
}

/// Load project keywords from `~/.imperium/identity/projects.yaml` if present.
/// Extracts `id` and `keywords` (or `description` words) per project entry.
pub fn load_imperium_projects(path: &Path) -> anyhow::Result<Vec<ProjectKeywords>> {
    let raw = std::fs::read_to_string(path)?;
    let doc: serde_yaml::Value = serde_yaml::from_str(&raw)?;
    let mut out = Vec::new();

    if let Some(seq) = doc.get("projects").and_then(|v| v.as_sequence()) {
        for entry in seq {
            let Some(id) = entry.get("id").and_then(|v| v.as_str()) else {
                continue;
            };
            let mut kws: Vec<String> = Vec::new();
            if let Some(list) = entry.get("keywords").and_then(|v| v.as_sequence()) {
                for k in list {
                    if let Some(s) = k.as_str() {
                        kws.push(s.to_string());
                    }
                }
            }
            if kws.is_empty() {
                if let Some(desc) = entry.get("description").and_then(|v| v.as_str()) {
                    kws.push(desc.to_string());
                }
                if let Some(name) = entry.get("name").and_then(|v| v.as_str()) {
                    kws.push(name.to_string());
                }
            }
            if !kws.is_empty() {
                out.push(ProjectKeywords {
                    project_id: id.to_string(),
                    keywords: kws,
                });
            }
        }
    }
    Ok(out)
}

pub fn default_imperium_projects_path() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
        .join(".imperium")
        .join("identity")
        .join("projects.yaml")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn mk_item(title: &str, summary: &str) -> FeedItem {
        FeedItem {
            feed_id: "test".into(),
            guid: "1".into(),
            link: "https://x".into(),
            title: title.into(),
            summary: summary.into(),
            published_at: Some(Utc::now()),
        }
    }

    #[test]
    fn score_zero_when_no_keywords() {
        let item = mk_item("Anything", "Body");
        assert_eq!(score_item(&item, &[]), 0.0);
    }

    #[test]
    fn score_zero_when_no_overlap() {
        let item = mk_item("Cats and dogs", "playing in the yard");
        let s = score_item(&item, &["quantum".into(), "supersymmetry".into()]);
        assert!(s < 0.01);
    }

    #[test]
    fn score_positive_on_match() {
        let item = mk_item("New Rust release v1.80", "compiler improvements");
        let s = score_item(&item, &["rust".into(), "compiler".into()]);
        assert!(s > 0.0);
    }

    #[test]
    fn score_full_match() {
        let item = mk_item(
            "agent orchestration with rust and sqlite",
            "agent orchestration with rust and sqlite",
        );
        let s = score_item(&item, &["rust".into(), "sqlite".into(), "agent".into()]);
        assert!((s - 1.0).abs() < 1e-3);
    }

    #[test]
    fn matching_projects_picks_above_threshold() {
        let item = mk_item(
            "Embedding models compared",
            "Gemini, OpenAI, Cohere benchmarks",
        );
        let projects = vec![
            ProjectKeywords {
                project_id: "altevra".into(),
                keywords: vec!["embedding".into(), "gemini".into(), "models".into()],
            },
            ProjectKeywords {
                project_id: "unrelated".into(),
                keywords: vec!["pasta".into(), "tomato".into()],
            },
        ];
        let (max, matched) = matching_projects(&item, &projects, 0.3);
        assert!(max > 0.5);
        assert!(matched.iter().any(|p| p == "altevra"));
        assert!(!matched.iter().any(|p| p == "unrelated"));
    }

    #[test]
    fn stopwords_filtered() {
        let toks = tokenize("The quick brown fox jumps over the lazy dog");
        assert!(!toks.contains(&"the".to_string()));
        assert!(toks.contains(&"quick".to_string()));
    }

    #[test]
    fn load_imperium_projects_parses_yaml() {
        let yaml = r#"
projects:
  - id: altevra
    name: Altevra
    description: Agent OS for AI tools
    keywords: [rust, agent, sqlite, embeddings]
  - id: revesta
    name: ReVesta
    description: Real estate lead pipeline
"#;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), yaml).unwrap();
        let projects = load_imperium_projects(tmp.path()).unwrap();
        assert_eq!(projects.len(), 2);
        let altevra = &projects[0];
        assert_eq!(altevra.project_id, "altevra");
        assert!(altevra.keywords.iter().any(|k| k == "rust"));
        let revesta = &projects[1];
        // ReVesta has no keywords list — should fallback to description/name.
        assert!(!revesta.keywords.is_empty());
    }
}

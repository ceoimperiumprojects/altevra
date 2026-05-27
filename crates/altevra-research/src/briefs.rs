//! Brief writers — daily Obsidian summary + per-project vault brief.
//!
//! Both writers are idempotent on the same date: re-running on the same day
//! overwrites the file (file is keyed by `YYYY-MM-DD-altevra-brief.md`).

use chrono::Utc;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::fetcher::FeedItem;

#[derive(Debug, Clone)]
pub struct ScoredItem {
    pub item: FeedItem,
    pub score: f32,
    pub matched_projects: Vec<String>,
}

/// Write the global daily brief into `daily_obsidian_dir/YYYY-MM-DD-altevra-brief.md`.
/// Items are grouped by feed category if available.
pub fn write_daily_brief(
    daily_obsidian_dir: &Path,
    items: &[ScoredItem],
) -> anyhow::Result<PathBuf> {
    let date = Utc::now().format("%Y-%m-%d").to_string();
    let path = daily_obsidian_dir.join(format!("{date}-altevra-brief.md"));
    std::fs::create_dir_all(daily_obsidian_dir)?;

    let source_count = items
        .iter()
        .map(|i| i.item.feed_id.as_str())
        .collect::<std::collections::HashSet<_>>()
        .len();

    let mut out = String::new();
    out.push_str("---\n");
    out.push_str("kind: research-brief\n");
    out.push_str("generated_by: altevra-research\n");
    out.push_str(&format!(
        "generated_at: {}\n",
        Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    ));
    out.push_str(&format!("date: {date}\n"));
    out.push_str(&format!("items_count: {}\n", items.len()));
    out.push_str(&format!("source_count: {source_count}\n"));
    out.push_str("---\n\n");
    out.push_str(&format!("# Altevra Daily Brief — {date}\n\n"));

    if items.is_empty() {
        out.push_str("_No items fetched in window. Either feeds are quiet or all sources are within their fetch interval._\n");
    } else {
        let mut by_feed: BTreeMap<&str, Vec<&ScoredItem>> = BTreeMap::new();
        for it in items {
            by_feed.entry(&it.item.feed_id).or_default().push(it);
        }
        out.push_str(&format!(
            "**{}** items across **{}** feeds.\n\n",
            items.len(),
            by_feed.len()
        ));
        for (feed, list) in &by_feed {
            out.push_str(&format!("## {feed}\n\n"));
            for it in list {
                let title = if it.item.title.is_empty() {
                    "(no title)".to_string()
                } else {
                    it.item.title.clone()
                };
                out.push_str(&format!("- [{}]({})", title, it.item.link));
                if !it.matched_projects.is_empty() {
                    out.push_str(&format!(" — _{}_", it.matched_projects.join(", ")));
                }
                if it.score > 0.0 {
                    out.push_str(&format!(" `score={:.2}`", it.score));
                }
                out.push('\n');
                if !it.item.summary.is_empty() {
                    let snippet = truncate(&it.item.summary, 240);
                    out.push_str(&format!("  > {snippet}\n"));
                }
            }
            out.push('\n');
        }
    }

    atomic_write(&path, &out)?;
    Ok(path)
}

/// Write a per-project brief into `vault_root/<project_vault>/YYYY-MM-DD-brief.md`.
/// Only items where `project_id` is in `matched_projects` are included.
pub fn write_project_brief(
    vault_root: &Path,
    project_vault_subdir: &Path,
    project_id: &str,
    items: &[ScoredItem],
) -> anyhow::Result<Option<PathBuf>> {
    let relevant: Vec<&ScoredItem> = items
        .iter()
        .filter(|i| i.matched_projects.iter().any(|p| p == project_id))
        .collect();

    if relevant.is_empty() {
        return Ok(None);
    }

    let date = Utc::now().format("%Y-%m-%d").to_string();
    let dir = vault_root.join(project_vault_subdir);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{date}-brief.md"));

    let mut out = String::new();
    out.push_str("---\n");
    out.push_str("kind: research-brief\n");
    out.push_str("generated_by: altevra-research\n");
    out.push_str(&format!("project: {project_id}\n"));
    out.push_str(&format!(
        "generated_at: {}\n",
        Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    ));
    out.push_str(&format!("date: {date}\n"));
    out.push_str(&format!("items_count: {}\n", relevant.len()));
    out.push_str("---\n\n");
    out.push_str(&format!("# Research Brief — {project_id} — {date}\n\n"));

    let mut sorted: Vec<&ScoredItem> = relevant.iter().copied().collect();
    sorted.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    for it in sorted {
        let title = if it.item.title.is_empty() {
            "(no title)".to_string()
        } else {
            it.item.title.clone()
        };
        out.push_str(&format!(
            "- [{}]({}) `score={:.2}` _{}_\n",
            title, it.item.link, it.score, it.item.feed_id
        ));
        if !it.item.summary.is_empty() {
            out.push_str(&format!("  > {}\n", truncate(&it.item.summary, 300)));
        }
    }

    atomic_write(&path, &out)?;
    Ok(Some(path))
}

fn atomic_write(path: &Path, content: &str) -> anyhow::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn item(feed: &str, title: &str, projects: Vec<&str>, score: f32) -> ScoredItem {
        ScoredItem {
            item: FeedItem {
                feed_id: feed.into(),
                guid: format!("g-{title}"),
                link: format!("https://ex.com/{title}"),
                title: title.into(),
                summary: format!("Summary of {title}"),
                published_at: Some(Utc::now()),
            },
            score,
            matched_projects: projects.into_iter().map(String::from).collect(),
        }
    }

    #[test]
    fn daily_brief_writes_frontmatter() {
        let tmp = TempDir::new().unwrap();
        let items = vec![
            item("hn-frontpage", "Foo released v2", vec!["altevra"], 0.7),
            item("arxiv-cs-ai", "Bar paper", vec![], 0.3),
        ];
        let path = write_daily_brief(tmp.path(), &items).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.starts_with("---\n"));
        assert!(content.contains("kind: research-brief"));
        assert!(content.contains("items_count: 2"));
        assert!(content.contains("source_count: 2"));
        assert!(content.contains("Foo released v2"));
        assert!(content.contains("Bar paper"));
    }

    #[test]
    fn daily_brief_handles_empty() {
        let tmp = TempDir::new().unwrap();
        let path = write_daily_brief(tmp.path(), &[]).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("No items fetched"));
        assert!(content.contains("items_count: 0"));
    }

    #[test]
    fn project_brief_only_relevant_items() {
        let tmp = TempDir::new().unwrap();
        let items = vec![
            item("hn", "Match", vec!["altevra"], 0.8),
            item("hn", "Skip", vec!["other"], 0.5),
        ];
        let path = write_project_brief(tmp.path(), Path::new("05-research"), "altevra", &items)
            .unwrap()
            .expect("brief should be written");
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("Match"));
        assert!(!content.contains("Skip"));
        assert!(content.contains("items_count: 1"));
    }

    #[test]
    fn project_brief_returns_none_if_no_matches() {
        let tmp = TempDir::new().unwrap();
        let items = vec![item("hn", "Skip", vec!["other"], 0.5)];
        let res =
            write_project_brief(tmp.path(), Path::new("05-research"), "altevra", &items).unwrap();
        assert!(res.is_none());
    }

    #[test]
    fn briefs_are_idempotent_on_same_day() {
        let tmp = TempDir::new().unwrap();
        let items = vec![item("hn", "X", vec!["altevra"], 0.9)];
        let p1 = write_daily_brief(tmp.path(), &items).unwrap();
        let p2 = write_daily_brief(tmp.path(), &items).unwrap();
        assert_eq!(p1, p2);
    }
}

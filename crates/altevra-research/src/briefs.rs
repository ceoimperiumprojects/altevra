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

    let mut sorted: Vec<&ScoredItem> = relevant.to_vec();
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

/// One project section in the leverage brief.
#[derive(Debug, Clone)]
pub struct LeverageProject {
    pub project_id: String,
    pub priority: Option<String>,
    pub leverage_focus: Option<String>,
    pub items: Vec<ScoredItem>,
    /// LLM-distilled actionables (3-5 short bullets). When absent, fallback
    /// is the top-3 raw item titles.
    pub distilled_bullets: Vec<String>,
}

/// Write the daily *leverage* brief — projekt-first layout, with optional
/// LLM-distilled actionables per project. Goes to Pavle's Obsidian Briefs dir.
pub fn write_leverage_brief(
    daily_obsidian_dir: &Path,
    projects: &[LeverageProject],
) -> anyhow::Result<PathBuf> {
    let date = Utc::now().format("%Y-%m-%d").to_string();
    let path = daily_obsidian_dir.join(format!("{date}-leverage.md"));
    std::fs::create_dir_all(daily_obsidian_dir)?;

    let project_ids: Vec<String> = projects.iter().map(|p| p.project_id.clone()).collect();
    let total_items: usize = projects.iter().map(|p| p.items.len()).sum();

    let mut out = String::new();
    out.push_str("---\n");
    out.push_str("kind: leverage-brief\n");
    out.push_str("generated_by: altevra-brain\n");
    out.push_str(&format!(
        "generated_at: {}\n",
        Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    ));
    out.push_str(&format!("date: {date}\n"));
    out.push_str(&format!("projects_covered: [{}]\n", project_ids.join(", ")));
    out.push_str(&format!("total_items: {total_items}\n"));
    out.push_str("---\n\n");
    out.push_str(&format!("# Daily Leverage — {date}\n\n"));

    if projects.is_empty() {
        out.push_str(
            "_No projects swept today. Run `altevra brain start` to enable the daily sweep._\n",
        );
        atomic_write(&path, &out)?;
        return Ok(path);
    }

    for proj in projects {
        let heading = match &proj.priority {
            Some(p) => format!("## {} — {} ({} new)", proj.project_id, p, proj.items.len()),
            None => format!("## {} ({} new)", proj.project_id, proj.items.len()),
        };
        out.push_str(&heading);
        out.push_str("\n\n");
        if let Some(focus) = &proj.leverage_focus {
            out.push_str(&format!("_Focus: {focus}_\n\n"));
        }

        if !proj.distilled_bullets.is_empty() {
            out.push_str("**What might help today:**\n");
            for b in &proj.distilled_bullets {
                out.push_str(&format!("- {b}\n"));
            }
            out.push('\n');
        }

        if proj.items.is_empty() {
            out.push_str("_No new items pulled for this project today._\n\n");
            continue;
        }

        out.push_str("<details><summary>Sources</summary>\n\n");
        // Highest score first.
        let mut sorted: Vec<&ScoredItem> = proj.items.iter().collect();
        sorted.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        for it in sorted.iter().take(10) {
            let title = if it.item.title.is_empty() {
                "(no title)".to_string()
            } else {
                it.item.title.clone()
            };
            out.push_str(&format!(
                "- [{}]({}) `score={:.2}` _{}_\n",
                title, it.item.link, it.score, it.item.feed_id
            ));
        }
        out.push_str("\n</details>\n\n");
    }

    atomic_write(&path, &out)?;
    Ok(path)
}

/// Fallback distillation when no LLM is available: take top 3 item titles.
pub fn distill_fallback(items: &[ScoredItem]) -> Vec<String> {
    let mut sorted: Vec<&ScoredItem> = items.iter().collect();
    sorted.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    sorted
        .iter()
        .take(3)
        .map(|i| {
            if i.item.title.is_empty() {
                i.item.link.clone()
            } else {
                format!("{} ({})", i.item.title, i.item.feed_id)
            }
        })
        .collect()
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

    #[test]
    fn leverage_brief_renders_project_sections() {
        let tmp = TempDir::new().unwrap();
        let projects = vec![
            LeverageProject {
                project_id: "revesta".into(),
                priority: Some("P0".into()),
                leverage_focus: Some("GTM angles".into()),
                items: vec![
                    item("hn-frontpage", "Wasteless Series A", vec!["revesta"], 0.85),
                    item("techcrunch-ai", "Food tech roundup", vec!["revesta"], 0.5),
                ],
                distilled_bullets: vec![
                    "Competitor move: Wasteless raised $4M Series A".into(),
                    "GTM signal: 2 new Miami restaurants posted surplus inventory".into(),
                ],
            },
            LeverageProject {
                project_id: "altevra".into(),
                priority: Some("P1".into()),
                leverage_focus: None,
                items: vec![item(
                    "arxiv-cs-ai",
                    "Continuous embeddings",
                    vec!["altevra"],
                    0.7,
                )],
                distilled_bullets: vec![],
            },
        ];
        let path = write_leverage_brief(tmp.path(), &projects).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("kind: leverage-brief"));
        assert!(body.contains("projects_covered: [revesta, altevra]"));
        assert!(body.contains("## revesta — P0"));
        assert!(body.contains("Wasteless Series A"));
        assert!(body.contains("Competitor move"));
        assert!(body.contains("## altevra — P1"));
    }

    #[test]
    fn leverage_brief_handles_empty_projects() {
        let tmp = TempDir::new().unwrap();
        let path = write_leverage_brief(tmp.path(), &[]).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("No projects swept today"));
    }

    #[test]
    fn distill_fallback_uses_top_three_titles() {
        let items = vec![
            item("a", "alpha", vec!["x"], 0.9),
            item("b", "beta", vec!["x"], 0.3),
            item("c", "gamma", vec!["x"], 0.7),
            item("d", "delta", vec!["x"], 0.1),
        ];
        let bullets = distill_fallback(&items);
        assert_eq!(bullets.len(), 3);
        assert!(bullets[0].contains("alpha"));
        assert!(bullets[1].contains("gamma"));
        // delta should not make the cut (score 0.1 vs beta 0.3)
        assert!(bullets[2].contains("beta"));
    }
}

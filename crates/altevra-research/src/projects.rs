//! Per-project research agent — pulls keywords + queries from the Imperium
//! identity registry, layered over by per-project YAML overrides at
//! `~/.altevra/research/projects/<id>.yaml`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectAgent {
    pub project_id: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub queries: Vec<String>,
    #[serde(default = "default_sources_enabled")]
    pub sources_enabled: Vec<String>,
    #[serde(default = "default_budget")]
    pub daily_budget_queries: u32,
    #[serde(default = "default_scrape_budget")]
    pub daily_budget_scrapes: u32,
    #[serde(default)]
    pub leverage_focus: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
}

fn default_sources_enabled() -> Vec<String> {
    vec!["rss".into(), "github_trending".into(), "web_search".into()]
}

fn default_budget() -> u32 {
    5
}

fn default_scrape_budget() -> u32 {
    20
}

impl ProjectAgent {
    pub fn override_dir() -> PathBuf {
        std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/tmp"))
            .join(".altevra")
            .join("research")
            .join("projects")
    }

    /// Load a per-project override YAML if present.
    pub fn load_override(dir: &Path, project_id: &str) -> Option<Self> {
        let p = dir.join(format!("{project_id}.yaml"));
        if !p.exists() {
            return None;
        }
        let raw = std::fs::read_to_string(&p).ok()?;
        serde_yaml::from_str(&raw).ok()
    }

    /// Default budget by Pavle's project priority — P0=10, P1=5, P2/P3=3.
    pub fn budget_for_priority(p: Option<&str>) -> u32 {
        match p {
            Some("P0") => 10,
            Some("P1") => 5,
            Some("P2") | Some("P3") => 3,
            _ => 5,
        }
    }

    /// Derive a baseline ProjectAgent from a `~/.imperium/identity/projects.yaml`
    /// entry (a `serde_yaml::Value` of one project). The override YAML, if any,
    /// is merged on top.
    pub fn from_imperium_entry(entry: &serde_yaml::Value) -> Option<Self> {
        let id = entry.get("id").and_then(|v| v.as_str())?.to_string();
        let name = entry.get("name").and_then(|v| v.as_str()).map(String::from);
        let priority = entry
            .get("priority")
            .and_then(|v| v.as_str())
            .map(String::from);
        let description = entry
            .get("description")
            .and_then(|v| v.as_str())
            .map(String::from);

        let mut keywords: Vec<String> = entry
            .get("keywords")
            .and_then(|v| v.as_sequence())
            .map(|seq| {
                seq.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if keywords.is_empty() {
            if let Some(n) = &name {
                keywords.push(n.clone());
            }
            if let Some(d) = &description {
                keywords.push(d.clone());
            }
        }
        let queries: Vec<String> = entry
            .get("queries")
            .and_then(|v| v.as_sequence())
            .map(|seq| {
                seq.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        Some(Self {
            project_id: id,
            keywords,
            queries,
            sources_enabled: default_sources_enabled(),
            daily_budget_queries: Self::budget_for_priority(priority.as_deref()),
            daily_budget_scrapes: 20,
            leverage_focus: None,
            priority,
        })
    }

    /// Load all project agents from imperium identity + apply per-project overrides.
    pub fn load_all(imperium_projects_path: &Path) -> anyhow::Result<Vec<ProjectAgent>> {
        let raw = std::fs::read_to_string(imperium_projects_path)?;
        let doc: serde_yaml::Value = serde_yaml::from_str(&raw)?;
        let mut out = Vec::new();
        let Some(seq) = doc.get("projects").and_then(|v| v.as_sequence()) else {
            return Ok(out);
        };
        let override_dir = Self::override_dir();
        for entry in seq {
            let Some(mut agent) = Self::from_imperium_entry(entry) else {
                continue;
            };
            if let Some(ov) = Self::load_override(&override_dir, &agent.project_id) {
                // Override wins on every populated field.
                if !ov.keywords.is_empty() {
                    agent.keywords = ov.keywords;
                }
                if !ov.queries.is_empty() {
                    agent.queries = ov.queries;
                }
                if !ov.sources_enabled.is_empty() {
                    agent.sources_enabled = ov.sources_enabled;
                }
                if ov.daily_budget_queries != default_budget() {
                    agent.daily_budget_queries = ov.daily_budget_queries;
                }
                if ov.daily_budget_scrapes != default_scrape_budget() {
                    agent.daily_budget_scrapes = ov.daily_budget_scrapes;
                }
                if ov.leverage_focus.is_some() {
                    agent.leverage_focus = ov.leverage_focus;
                }
            }
            // If neither imperium nor override gave us queries, fall back to keywords as queries.
            if agent.queries.is_empty() {
                agent.queries = agent.keywords.clone();
            }
            out.push(agent);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn budget_for_priority_maps_pavle_tiers() {
        assert_eq!(ProjectAgent::budget_for_priority(Some("P0")), 10);
        assert_eq!(ProjectAgent::budget_for_priority(Some("P1")), 5);
        assert_eq!(ProjectAgent::budget_for_priority(Some("P2")), 3);
        assert_eq!(ProjectAgent::budget_for_priority(Some("P3")), 3);
        assert_eq!(ProjectAgent::budget_for_priority(None), 5);
    }

    #[test]
    fn from_imperium_entry_fills_keywords_from_name_when_absent() {
        let entry: serde_yaml::Value = serde_yaml::from_str(
            r#"
id: revesta
name: ReVesta
priority: P0
description: B2B food surplus marketplace
"#,
        )
        .unwrap();
        let agent = ProjectAgent::from_imperium_entry(&entry).unwrap();
        assert_eq!(agent.project_id, "revesta");
        assert!(agent.keywords.iter().any(|k| k.contains("ReVesta")));
        assert!(agent.keywords.iter().any(|k| k.contains("food surplus")));
        assert_eq!(agent.daily_budget_queries, 10); // P0 = 10
    }

    #[test]
    fn from_imperium_entry_uses_explicit_keywords_when_present() {
        let entry: serde_yaml::Value = serde_yaml::from_str(
            r#"
id: altevra
name: Altevra
priority: P1
keywords: [rust, mcp, agent]
queries: ["rust agent framework 2026"]
"#,
        )
        .unwrap();
        let agent = ProjectAgent::from_imperium_entry(&entry).unwrap();
        assert_eq!(agent.keywords, vec!["rust", "mcp", "agent"]);
        assert_eq!(agent.queries.len(), 1);
        assert_eq!(agent.daily_budget_queries, 5);
    }

    #[test]
    fn load_all_with_no_overrides_returns_agents() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("projects.yaml");
        std::fs::write(
            &path,
            r#"
projects:
  - id: revesta
    name: ReVesta
    priority: P0
  - id: altevra
    name: Altevra
    priority: P1
    keywords: [rust, mcp]
"#,
        )
        .unwrap();
        let agents = ProjectAgent::load_all(&path).unwrap();
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0].project_id, "revesta");
        assert_eq!(agents[0].daily_budget_queries, 10);
        assert_eq!(agents[1].keywords, vec!["rust", "mcp"]);
    }

    #[test]
    fn override_loader_returns_none_when_missing() {
        let tmp = TempDir::new().unwrap();
        let res = ProjectAgent::load_override(tmp.path(), "nope");
        assert!(res.is_none());
    }

    #[test]
    fn override_loader_parses_when_present() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("revesta.yaml");
        std::fs::write(
            &p,
            r#"
project_id: revesta
keywords: [food, restaurant, B2B]
queries: ["restaurant inventory AI"]
daily_budget_queries: 12
"#,
        )
        .unwrap();
        let ov = ProjectAgent::load_override(tmp.path(), "revesta").unwrap();
        assert_eq!(ov.keywords, vec!["food", "restaurant", "B2B"]);
        assert_eq!(ov.daily_budget_queries, 12);
    }
}

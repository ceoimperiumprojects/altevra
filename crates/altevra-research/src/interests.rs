//! Relevance gate (P4, CLAUDE.md §3.3) — research/surfacing only on STATED
//! interests + active goals, never on every passing keyword.
//!
//! The gate is a stated-preferences layer above the research engine:
//! `~/.altevra/interests.yaml` holds the interests Pavle has explicitly opted
//! into (keywords/domains per interest). Project keywords from the registry
//! and active goals are merged in at call time. An item that matches neither
//! an interest, a goal, nor a project keyword is DROPPED from the candidate
//! set (debug-logged, never silently scored into the feed).
//!
//! When the file holds no enabled interests (fresh template) the gate is
//! INACTIVE — existing project-keyword behavior is preserved so a fresh
//! install isn't silently mute. "No Minecraft modpack research" starts the
//! moment the first interest is stated.

use std::path::{Path, PathBuf};

/// One stated interest: a name plus the keywords/domains that identify it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Interest {
    pub name: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    /// Optional life-domain tags (business/personal/learning/...) — metadata
    /// for briefing grouping; not used for matching.
    #[serde(default)]
    pub domains: Vec<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct InterestsFile {
    #[serde(default)]
    interests: Vec<Interest>,
}

/// The loaded relevance gate. Stated interests from interests.yaml plus any
/// active goals merged via [`RelevanceGate::with_goals`].
#[derive(Debug, Clone, Default)]
pub struct RelevanceGate {
    interests: Vec<Interest>,
}

/// Commented template written on first touch (create-if-absent). All entries
/// are examples and commented out — the gate stays inactive until Pavle
/// states a real interest.
pub const INTERESTS_TEMPLATE: &str = r#"# Altevra relevance gate — stated interests (P4, CLAUDE.md §3.3).
#
# Research and proactive surfacing run ONLY on interests stated here plus
# active goals. Anything else is dropped as noise ("no Minecraft modpack
# research"). Uncomment / add entries to opt in.
#
# interests:
#   - name: rust-agents
#     keywords: [rust, agent, sqlite, embeddings, mcp]
#     domains: [business]
#   - name: nils-frahm-releases
#     keywords: [nils frahm, piano, ambient release]
#     domains: [personal]
#     enabled: true
interests: []
"#;

/// Default location: `~/.altevra/interests.yaml`.
pub fn default_interests_path() -> PathBuf {
    altevra_core::home_dir().join(".altevra/interests.yaml")
}

impl RelevanceGate {
    /// Load the gate from `path`. If the file is absent, write the commented
    /// template first (create-if-absent), then return an inactive gate.
    /// Unparsable YAML degrades to an inactive gate (never a hard failure —
    /// a broken interests file must not kill the research pipeline).
    pub fn load_or_create(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, INTERESTS_TEMPLATE)?;
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path)?;
        let file: InterestsFile = serde_yaml::from_str(&raw).unwrap_or_else(|e| {
            tracing::warn!("interests.yaml unparsable ({e}); relevance gate inactive");
            InterestsFile::default()
        });
        Ok(Self {
            interests: file.interests.into_iter().filter(|i| i.enabled).collect(),
        })
    }

    /// Build a gate from in-memory interests (tests, programmatic use).
    pub fn from_interests(interests: Vec<Interest>) -> Self {
        Self {
            interests: interests.into_iter().filter(|i| i.enabled).collect(),
        }
    }

    /// Merge active goals in as ad-hoc interests (each goal title is its own
    /// keyword set). "Stated interests + active goals" is the full gate.
    pub fn with_goals<S: AsRef<str>>(mut self, goal_titles: &[S]) -> Self {
        for t in goal_titles {
            let t = t.as_ref().trim();
            if t.is_empty() {
                continue;
            }
            self.interests.push(Interest {
                name: format!("goal:{t}"),
                keywords: vec![t.to_string()],
                domains: vec![],
                enabled: true,
            });
        }
        self
    }

    /// Is the gate active? False when nothing is stated (fresh template) —
    /// callers preserve their legacy behavior in that case.
    pub fn is_active(&self) -> bool {
        !self.interests.is_empty()
    }

    /// Does `text` match a stated interest? Returns the first matching
    /// interest name. Matching: every whitespace-separated token of one
    /// keyword must appear in the lowercased text (so "nils frahm" requires
    /// both words; single-word keywords are simple containment with a
    /// 3-char minimum to avoid stopword-ish noise).
    pub fn matching_interest(&self, text: &str) -> Option<&str> {
        let hay = text.to_lowercase();
        for interest in &self.interests {
            for kw in &interest.keywords {
                let kw = kw.trim().to_lowercase();
                if kw.len() < 3 {
                    continue;
                }
                if kw.split_whitespace().all(|tok| hay.contains(tok)) {
                    return Some(&interest.name);
                }
            }
        }
        None
    }

    /// The gate verdict for a candidate text. Inactive gate → allowed
    /// (legacy behavior preserved). Active gate → allowed only on a match.
    pub fn allows(&self, text: &str) -> bool {
        if !self.is_active() {
            return true;
        }
        self.matching_interest(text).is_some()
    }
}

/// Candidate filter for the research feed path (the scoring loop in
/// `run_research_fetcher`). An item passes when:
///   * it matched a project above the relevance threshold (active goals
///     proxy — projects ARE the stated active work), OR
///   * the relevance gate matches a stated interest.
/// With an ACTIVE gate, anything else is dropped + debug-logged. With an
/// inactive gate everything passes (legacy behavior).
pub fn gate_allows_item(
    gate: &RelevanceGate,
    title: &str,
    summary: &str,
    matched_projects: &[String],
) -> bool {
    if !gate.is_active() {
        return true;
    }
    if !matched_projects.is_empty() {
        return true;
    }
    let text = format!("{title} {summary}");
    if gate.matching_interest(&text).is_some() {
        return true;
    }
    tracing::debug!("relevance gate dropped off-interest item: {title:?}");
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_created_when_absent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join(".altevra/interests.yaml");
        let gate = RelevanceGate::load_or_create(&path).unwrap();
        assert!(path.exists(), "interests.yaml template must be created");
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("interests:"), "template must be commented YAML");
        assert!(!gate.is_active(), "fresh template → inactive gate");
        // Second load parses the template cleanly.
        let gate2 = RelevanceGate::load_or_create(&path).unwrap();
        assert!(!gate2.is_active());
    }

    #[test]
    fn stated_interest_matches_and_off_interest_dropped() {
        let gate = RelevanceGate::from_interests(vec![Interest {
            name: "rust-agents".into(),
            keywords: vec!["rust".into(), "sqlite".into()],
            domains: vec![],
            enabled: true,
        }]);
        assert!(gate.is_active());
        assert_eq!(
            gate.matching_interest("New Rust 1.80 release notes"),
            Some("rust-agents")
        );
        assert!(gate.allows("sqlite WAL deep dive"));
        // Off-interest → dropped.
        assert!(!gate.allows("Top 10 Minecraft modpacks of 2026"));
        assert!(!gate_allows_item(
            &gate,
            "Top 10 Minecraft modpacks",
            "blocky fun",
            &[]
        ));
        // Project match passes even without an interest keyword hit.
        assert!(gate_allows_item(
            &gate,
            "Anything",
            "at all",
            &["revesta".to_string()]
        ));
    }

    #[test]
    fn goals_merge_into_gate() {
        let gate =
            RelevanceGate::from_interests(vec![]).with_goals(&["close two Simple Surplus clients"]);
        assert!(gate.is_active());
        assert!(gate.allows("Simple Surplus pipeline — two new clients close"));
        assert!(!gate.allows("celebrity gossip roundup"));
    }

    #[test]
    fn disabled_interest_ignored_and_multiword_requires_all_tokens() {
        let gate = RelevanceGate::from_interests(vec![
            Interest {
                name: "off".into(),
                keywords: vec!["quantum".into()],
                domains: vec![],
                enabled: false,
            },
            Interest {
                name: "nils".into(),
                keywords: vec!["nils frahm".into()],
                domains: vec!["personal".into()],
                enabled: true,
            },
        ]);
        assert!(!gate.allows("quantum computing weekly"));
        assert!(gate.allows("New Nils Frahm album announced"));
        assert!(!gate.allows("Frahm valley tourism guide is nilly"));
    }
}

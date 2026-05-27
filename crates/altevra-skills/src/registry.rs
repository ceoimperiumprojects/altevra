use crate::checksum;
use crate::parser::{parse_skill, ParsedSkill};
use crate::version::SkillVersion;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct SkillRegistryEntry {
    pub id: Uuid,
    pub skill: ParsedSkill,
    pub checksum: String,
    pub source_path: String,
    pub registered_at: DateTime<Utc>,
}

impl SkillRegistryEntry {
    pub fn version(&self) -> Option<SkillVersion> {
        self.skill.version()
    }

    pub fn slug(&self) -> &str {
        self.skill.slug()
    }

    pub fn is_drift(&self) -> bool {
        !checksum::verify(&self.skill.raw, &self.checksum)
    }
}

/// In-memory skill registry (loaded from vault on startup).
#[derive(Debug, Default)]
pub struct SkillRegistry {
    entries: HashMap<String, SkillRegistryEntry>,
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a skill from raw markdown content.
    pub fn register(
        &mut self,
        source_path: impl Into<String>,
        content: &str,
    ) -> anyhow::Result<&SkillRegistryEntry> {
        let skill = parse_skill(content)?;
        let slug = skill.slug().to_string();
        let entry = SkillRegistryEntry {
            id: Uuid::new_v4(),
            checksum: checksum::compute(content),
            source_path: source_path.into(),
            registered_at: Utc::now(),
            skill,
        };
        self.entries.insert(slug.clone(), entry);
        Ok(self.entries.get(&slug).unwrap())
    }

    pub fn get(&self, slug: &str) -> Option<&SkillRegistryEntry> {
        self.entries.get(slug)
    }

    pub fn list(&self) -> Vec<&SkillRegistryEntry> {
        let mut entries: Vec<_> = self.entries.values().collect();
        entries.sort_by(|a, b| a.slug().cmp(b.slug()));
        entries
    }

    pub fn check_version(&self, slug: &str, installed_version: &str) -> VersionCheckResult {
        let Some(entry) = self.get(slug) else {
            return VersionCheckResult::NotFound;
        };
        let Some(latest) = entry.version() else {
            return VersionCheckResult::ParseError;
        };
        let Ok(installed) = installed_version.parse::<SkillVersion>() else {
            return VersionCheckResult::ParseError;
        };

        if installed == latest {
            VersionCheckResult::Current
        } else if installed < latest {
            VersionCheckResult::Outdated {
                installed: installed.to_string(),
                latest: latest.to_string(),
            }
        } else {
            VersionCheckResult::Ahead {
                installed: installed.to_string(),
                latest: latest.to_string(),
            }
        }
    }

    /// Like `check_version` but treats `None` as "not installed" rather than comparing "0.0.0".
    pub fn check_version_opt(
        &self,
        slug: &str,
        installed_version: Option<&str>,
    ) -> VersionCheckResult {
        match installed_version {
            Some(v) => self.check_version(slug, v),
            None => {
                if self.get(slug).is_some() {
                    VersionCheckResult::NotInstalled
                } else {
                    VersionCheckResult::NotFound
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionCheckResult {
    Current,
    Outdated {
        installed: String,
        latest: String,
    },
    Ahead {
        installed: String,
        latest: String,
    },
    /// Skill is known in the registry but no installed version was provided.
    NotInstalled,
    /// Skill slug is not in the registry at all.
    NotFound,
    ParseError,
}

impl std::fmt::Display for VersionCheckResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Current => write!(f, "current"),
            Self::Outdated { installed, latest } => {
                write!(f, "outdated (installed: {installed}, latest: {latest})")
            }
            Self::Ahead { installed, latest } => {
                write!(f, "ahead (installed: {installed}, latest: {latest})")
            }
            Self::NotInstalled => write!(f, "not installed"),
            Self::NotFound => write!(f, "not found"),
            Self::ParseError => write!(f, "version parse error"),
        }
    }
}

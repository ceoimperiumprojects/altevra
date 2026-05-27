use altevra_skills::registry::{SkillRegistry, VersionCheckResult};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillFreshnessStatus {
    Current,
    Outdated,
    NotInstalled,
    Unknown,
}

impl std::fmt::Display for SkillFreshnessStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Current => write!(f, "current"),
            Self::Outdated => write!(f, "outdated"),
            Self::NotInstalled => write!(f, "not_installed"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FreshnessCheck {
    pub skill_slug: String,
    pub installed_version: Option<String>,
    pub latest_version: Option<String>,
    pub status: SkillFreshnessStatus,
    pub action_required: Option<String>,
}

impl FreshnessCheck {
    pub fn check(registry: &SkillRegistry, slug: &str, installed_version: Option<&str>) -> Self {
        let latest_version = registry
            .get(slug)
            .and_then(|e| e.version())
            .map(|v| v.to_string());

        match installed_version {
            None => Self {
                skill_slug: slug.to_string(),
                installed_version: None,
                latest_version,
                status: SkillFreshnessStatus::NotInstalled,
                action_required: Some(format!(
                    "Run: altevra connect --tool <tool> to install {slug}"
                )),
            },
            Some(installed) => {
                let check = registry.check_version(slug, installed);
                let status = match &check {
                    VersionCheckResult::Current => SkillFreshnessStatus::Current,
                    VersionCheckResult::Outdated { .. } => SkillFreshnessStatus::Outdated,
                    _ => SkillFreshnessStatus::Unknown,
                };
                let action_required = if status == SkillFreshnessStatus::Outdated {
                    Some(format!("Run: altevra skill refresh {slug}"))
                } else {
                    None
                };
                Self {
                    skill_slug: slug.to_string(),
                    installed_version: Some(installed.to_string()),
                    latest_version,
                    status,
                    action_required,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_freshness_not_installed() {
        let registry = SkillRegistry::new();
        let check = FreshnessCheck::check(&registry, "altevra-core", None);
        assert_eq!(check.status, SkillFreshnessStatus::NotInstalled);
        assert!(check.action_required.is_some());
    }

    #[test]
    fn test_freshness_current() {
        let mut registry = SkillRegistry::new();
        let skill_content = "---\nslug: altevra-core\nversion: 0.5.0\ntitle: Test\n---\nbody";
        registry.register("test.md", skill_content).unwrap();
        let check = FreshnessCheck::check(&registry, "altevra-core", Some("0.5.0"));
        assert_eq!(check.status, SkillFreshnessStatus::Current);
        assert!(check.action_required.is_none());
    }

    #[test]
    fn test_freshness_outdated() {
        let mut registry = SkillRegistry::new();
        let skill_content = "---\nslug: altevra-core\nversion: 0.5.0\ntitle: Test\n---\nbody";
        registry.register("test.md", skill_content).unwrap();
        let check = FreshnessCheck::check(&registry, "altevra-core", Some("0.4.0"));
        assert_eq!(check.status, SkillFreshnessStatus::Outdated);
        assert!(check.action_required.is_some());
    }
}

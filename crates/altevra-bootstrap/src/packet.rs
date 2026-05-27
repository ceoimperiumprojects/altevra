use crate::freshness::FreshnessCheck;
use crate::setup_status::SetupStatus;
use altevra_core::updates::UpdateFeedItem;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The bootstrap packet delivered to an agent at session start.
/// Contains everything needed to begin work with full context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentBootstrapPacket {
    pub packet_id: Uuid,
    pub generated_at: DateTime<Utc>,
    pub tool_name: String,
    pub project: Option<String>,
    pub altevra_version: String,
    pub skill_freshness: Vec<FreshnessCheck>,
    pub setup_status: SetupStatus,
    pub last_updates: Vec<UpdateSummary>,
    pub active_task: Option<TaskPlaceholder>,
    pub goals: Vec<GoalPlaceholder>,
    pub warnings: Vec<String>,
    pub recommended_next_action: Option<String>,
    pub session_id: Uuid,
}

/// Lightweight summary of an update for the bootstrap packet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateSummary {
    pub title: String,
    pub importance: String,
    pub update_type: String,
    pub short_summary: String,
    pub created_at: DateTime<Utc>,
}

impl From<&UpdateFeedItem> for UpdateSummary {
    fn from(item: &UpdateFeedItem) -> Self {
        Self {
            title: item.title.clone(),
            importance: item.importance.to_string(),
            update_type: item.update_type.clone(),
            short_summary: item.short_summary.clone(),
            created_at: item.created_at,
        }
    }
}

/// Placeholder for task system (not yet implemented in v0.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPlaceholder {
    pub id: String,
    pub title: String,
    pub status: String,
}

/// Placeholder for goal system (not yet implemented in v0.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalPlaceholder {
    pub id: String,
    pub title: String,
}

pub struct BootstrapBuilder {
    tool_name: String,
    project: Option<String>,
    altevra_version: String,
    skill_freshness: Vec<FreshnessCheck>,
    setup_status: Option<SetupStatus>,
    last_updates: Vec<UpdateSummary>,
    warnings: Vec<String>,
}

impl BootstrapBuilder {
    pub fn new(tool_name: impl Into<String>, altevra_version: impl Into<String>) -> Self {
        Self {
            tool_name: tool_name.into(),
            project: None,
            altevra_version: altevra_version.into(),
            skill_freshness: vec![],
            setup_status: None,
            last_updates: vec![],
            warnings: vec![],
        }
    }

    pub fn project(mut self, project: impl Into<String>) -> Self {
        self.project = Some(project.into());
        self
    }

    pub fn skill_freshness(mut self, checks: Vec<FreshnessCheck>) -> Self {
        self.skill_freshness = checks;
        self
    }

    pub fn setup_status(mut self, status: SetupStatus) -> Self {
        self.setup_status = Some(status);
        self
    }

    pub fn last_updates(mut self, updates: Vec<UpdateFeedItem>) -> Self {
        self.last_updates = updates.iter().map(UpdateSummary::from).collect();
        self
    }

    pub fn warning(mut self, w: impl Into<String>) -> Self {
        self.warnings.push(w.into());
        self
    }

    pub fn build(self) -> AgentBootstrapPacket {
        let tool_name = self.tool_name.clone();
        let setup_status = self
            .setup_status
            .unwrap_or_else(|| SetupStatus::placeholder(&tool_name));

        let outdated: Vec<_> = self
            .skill_freshness
            .iter()
            .filter(|c| c.status == crate::freshness::SkillFreshnessStatus::Outdated)
            .collect();

        let mut warnings = self.warnings;
        for c in &outdated {
            warnings.push(format!(
                "Skill '{}' is outdated (installed: {}, latest: {})",
                c.skill_slug,
                c.installed_version.as_deref().unwrap_or("none"),
                c.latest_version.as_deref().unwrap_or("unknown"),
            ));
        }

        let recommended_next_action = if !outdated.is_empty() {
            Some("Run: altevra skill check --all and then altevra skill refresh".to_string())
        } else if !setup_status.components.is_empty() {
            Some("Run: altevra connect --tool <tool> to configure your environment".to_string())
        } else {
            None
        };

        AgentBootstrapPacket {
            packet_id: Uuid::new_v4(),
            generated_at: Utc::now(),
            tool_name,
            project: self.project,
            altevra_version: self.altevra_version,
            skill_freshness: self.skill_freshness,
            setup_status,
            last_updates: self.last_updates,
            active_task: None,
            goals: vec![],
            warnings,
            recommended_next_action,
            session_id: Uuid::new_v4(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::freshness::FreshnessCheck;
    use altevra_skills::registry::SkillRegistry;

    #[test]
    fn test_bootstrap_packet_builds() {
        let packet = BootstrapBuilder::new("claude-code", "0.1.0")
            .project("altevra")
            .build();

        assert_eq!(packet.tool_name, "claude-code");
        assert_eq!(packet.project.as_deref(), Some("altevra"));
        assert!(!packet.session_id.is_nil());
    }

    #[test]
    fn test_bootstrap_packet_json_stable() {
        let packet = BootstrapBuilder::new("claude-code", "0.1.0")
            .project("test")
            .build();

        let json = serde_json::to_string(&packet).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["tool_name"], "claude-code");
        assert_eq!(parsed["project"], "test");
        assert!(parsed["packet_id"].is_string());
        assert!(parsed["session_id"].is_string());
        assert!(parsed["last_updates"].is_array());
        assert!(parsed["warnings"].is_array());
    }

    #[test]
    fn test_bootstrap_outdated_skill_adds_warning() {
        let mut registry = SkillRegistry::new();
        let skill = "---\nslug: altevra-core\nversion: 0.5.0\ntitle: T\n---\nbody";
        registry.register("test.md", skill).unwrap();

        let freshness = vec![FreshnessCheck::check(
            &registry,
            "altevra-core",
            Some("0.4.0"),
        )];
        let packet = BootstrapBuilder::new("claude-code", "0.1.0")
            .skill_freshness(freshness)
            .build();

        assert!(!packet.warnings.is_empty());
        assert!(packet.warnings[0].contains("outdated"));
        assert!(packet.recommended_next_action.is_some());
    }
}

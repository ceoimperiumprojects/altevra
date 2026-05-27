use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::security::Sensitivity;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Importance {
    Noise,
    Low,
    Medium,
    High,
    Critical,
}

impl std::fmt::Display for Importance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Noise => write!(f, "noise"),
            Self::Low => write!(f, "low"),
            Self::Medium => write!(f, "medium"),
            Self::High => write!(f, "high"),
            Self::Critical => write!(f, "critical"),
        }
    }
}

impl std::str::FromStr for Importance {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "noise" => Ok(Self::Noise),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "critical" => Ok(Self::Critical),
            other => Err(format!("Unknown importance: {other}")),
        }
    }
}

/// Agent-friendly processed update derived from an Event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateFeedItem {
    pub id: Uuid,
    pub event_id: Uuid,
    pub project_id: Option<Uuid>,
    pub update_type: String,
    pub importance: Importance,
    pub title: String,
    pub short_summary: String,
    pub agent_summary: Option<String>,
    pub affected_entities: serde_json::Value,
    pub recommended_agent_action: Option<String>,
    pub visible_to_agents: bool,
    pub sensitivity: Sensitivity,
    pub created_at: DateTime<Utc>,
}

impl UpdateFeedItem {
    pub fn from_event(
        event_id: Uuid,
        update_type: impl Into<String>,
        importance: Importance,
        title: impl Into<String>,
        short_summary: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            event_id,
            project_id: None,
            update_type: update_type.into(),
            importance,
            title: title.into(),
            short_summary: short_summary.into(),
            agent_summary: None,
            affected_entities: serde_json::Value::Array(vec![]),
            recommended_agent_action: None,
            visible_to_agents: true,
            sensitivity: Sensitivity::Internal,
            created_at: Utc::now(),
        }
    }
}

/// Query filters for fetching updates.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdatesQuery {
    pub project_id: Option<Uuid>,
    pub since: Option<DateTime<Utc>>,
    pub importance_min: Option<Importance>,
    pub agent_id: Option<String>,
    pub limit: Option<i64>,
}

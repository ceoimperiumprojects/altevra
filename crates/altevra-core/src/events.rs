use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::security::Sensitivity;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    DocumentChanged,
    DocumentIndexed,
    SkillUpdated,
    SkillInstalled,
    SkillDriftDetected,
    TaskCreated,
    TaskUpdated,
    TaskCompleted,
    GoalCreated,
    GoalUpdated,
    ProjectStatusChanged,
    DecisionSaved,
    ResearchSaved,
    ResearchSynthesized,
    InsightCreated,
    HookInstalled,
    HookFailed,
    AdapterSynced,
    ConfigChanged,
    SessionStarted,
    SessionEnded,
    CapabilityAdded,
    ConnectorSynced,
    SecretChanged,
    ErrorLogged,
    ReviewItemCreated,
    ToolConnected,
    // v0.3 — Omniscient Brain OS observability events.
    ToolCallObserved,
    PromptSent,
    ResponseReceived,
    FileChanged,
    McpCall,
    AgentThinkingStep,
    // P3c — SkillOpt backward-pass signals (PLAN-ALIVE §P3c). Emitted by
    // hook_handle: an org/installed skill was invoked (PostToolUse on the
    // Skill tool) and the user "reacted" (a UserPromptSubmit inside the
    // K-message judgment window). The skill_reaction_judge brain job drains
    // pending invocations through the success judge.
    SkillInvocation,
    SkillReaction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorType {
    Agent,
    User,
    System,
    Adapter,
    Hook,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventStatus {
    Pending,
    Processed,
    Skipped,
    Error,
}

/// Core event — emitted by every meaningful Altevra action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: Uuid,
    pub event_type: EventType,
    pub project_id: Option<Uuid>,
    pub actor_type: ActorType,
    pub actor_id: Option<String>,
    pub source: String,
    pub entity_type: Option<String>,
    pub entity_id: Option<String>,
    pub title: String,
    pub summary: Option<String>,
    pub payload: serde_json::Value,
    pub sensitivity: Sensitivity,
    pub created_at: DateTime<Utc>,
    pub processed_at: Option<DateTime<Utc>>,
    pub status: EventStatus,
}

impl std::fmt::Display for EventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = serde_json::to_value(self)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| format!("{self:?}").to_lowercase());
        write!(f, "{s}")
    }
}

impl std::str::FromStr for EventType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_value(serde_json::Value::String(s.to_string())).map_err(|e| e.to_string())
    }
}

impl std::str::FromStr for ActorType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_value(serde_json::Value::String(s.to_string())).map_err(|e| e.to_string())
    }
}

impl std::fmt::Display for ActorType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Agent => "agent",
            Self::User => "user",
            Self::System => "system",
            Self::Adapter => "adapter",
            Self::Hook => "hook",
        };
        write!(f, "{s}")
    }
}

impl std::str::FromStr for EventStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_value(serde_json::Value::String(s.to_string())).map_err(|e| e.to_string())
    }
}

impl std::fmt::Display for EventStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Pending => "pending",
            Self::Processed => "processed",
            Self::Skipped => "skipped",
            Self::Error => "error",
        };
        write!(f, "{s}")
    }
}

impl Event {
    pub fn new(
        event_type: EventType,
        title: impl Into<String>,
        source: impl Into<String>,
        actor_type: ActorType,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            event_type,
            project_id: None,
            actor_type,
            actor_id: None,
            source: source.into(),
            entity_type: None,
            entity_id: None,
            title: title.into(),
            summary: None,
            payload: serde_json::Value::Object(Default::default()),
            sensitivity: Sensitivity::Internal,
            created_at: Utc::now(),
            processed_at: None,
            status: EventStatus::Pending,
        }
    }

    pub fn with_project(mut self, project_id: Uuid) -> Self {
        self.project_id = Some(project_id);
        self
    }

    pub fn with_actor(mut self, actor_id: impl Into<String>) -> Self {
        self.actor_id = Some(actor_id.into());
        self
    }

    pub fn with_entity(
        mut self,
        entity_type: impl Into<String>,
        entity_id: impl Into<String>,
    ) -> Self {
        self.entity_type = Some(entity_type.into());
        self.entity_id = Some(entity_id.into());
        self
    }

    pub fn with_payload(mut self, payload: serde_json::Value) -> Self {
        self.payload = payload;
        self
    }

    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }
}

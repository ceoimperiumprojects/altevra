use serde::{Deserialize, Serialize};

/// All possible hook actions that Altevra can execute.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookAction {
    CheckSkillVersion,
    GetLastUpdates,
    GetProjectContext,
    StartSessionLog,
    EndSessionLog,
    SummarizeSession,
    EmitEvent,
    ScheduleIngestion,
    CreatePendingChange,
    DetectSecretLeak,
    CreateReviewItem,
}

impl std::fmt::Display for HookAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = serde_json::to_value(self)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        write!(f, "{s}")
    }
}

impl std::str::FromStr for HookAction {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_value(serde_json::Value::String(s.to_string())).map_err(|e| e.to_string())
    }
}

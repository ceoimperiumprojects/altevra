use serde::{Deserialize, Serialize};

/// All hook event types Altevra understands universally.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UniversalHookType {
    SessionStart,
    SessionEnd,
    BeforeToolCall,
    AfterToolCall,
    BeforeFileEdit,
    AfterFileEdit,
    BeforeCommand,
    AfterCommand,
    OnError,
    OnSkillCheck,
    OnContextRequest,
    OnTaskComplete,
    OnProjectSwitch,
}

impl std::fmt::Display for UniversalHookType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = serde_json::to_value(self)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| format!("{self:?}").to_lowercase());
        write!(f, "{s}")
    }
}

impl std::str::FromStr for UniversalHookType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_value(serde_json::Value::String(s.to_string())).map_err(|e| e.to_string())
    }
}

/// A registered universal hook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UniversalHook {
    pub slug: String,
    pub version: String,
    pub hook_type: UniversalHookType,
    pub actions: Vec<String>,
    pub description: Option<String>,
    pub enabled: bool,
}

/// The payload carried by a hook event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookEvent {
    pub hook_type: UniversalHookType,
    pub tool_name: String,
    pub project: Option<String>,
    pub session_id: Option<String>,
    pub payload: serde_json::Value,
}

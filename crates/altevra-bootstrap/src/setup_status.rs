use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComponentStatus {
    Current,
    Outdated,
    Drifted,
    Missing,
    Conflicted,
    Unsupported,
}

impl std::fmt::Display for ComponentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Current => "current",
            Self::Outdated => "outdated",
            Self::Drifted => "drifted",
            Self::Missing => "missing",
            Self::Conflicted => "conflicted",
            Self::Unsupported => "unsupported",
        };
        write!(f, "{s}")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentCheck {
    pub component: String,
    pub status: ComponentStatus,
    pub path: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetupStatus {
    pub tool_name: String,
    pub overall: ComponentStatus,
    pub components: Vec<ComponentCheck>,
    pub warnings: Vec<String>,
    pub run_repair: bool,
}

impl SetupStatus {
    pub fn placeholder(tool_name: impl Into<String>) -> Self {
        Self {
            tool_name: tool_name.into(),
            overall: ComponentStatus::Current,
            components: vec![
                ComponentCheck {
                    component: "instruction_file".to_string(),
                    status: ComponentStatus::Missing,
                    path: Some(".claude/altevra-instructions.md".to_string()),
                    note: Some("Run altevra connect --tool claude-code to install".to_string()),
                },
                ComponentCheck {
                    component: "settings_json".to_string(),
                    status: ComponentStatus::Missing,
                    path: Some(".claude/settings.json".to_string()),
                    note: Some("Run altevra connect --tool claude-code to install".to_string()),
                },
            ],
            warnings: vec!["Setup not yet verified — run altevra connect".to_string()],
            run_repair: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_display() {
        assert_eq!(ComponentStatus::Current.to_string(), "current");
        assert_eq!(ComponentStatus::Drifted.to_string(), "drifted");
        assert_eq!(ComponentStatus::Missing.to_string(), "missing");
    }

    #[test]
    fn test_setup_placeholder() {
        let s = SetupStatus::placeholder("claude-code");
        assert_eq!(s.tool_name, "claude-code");
        assert_eq!(s.components.len(), 2);
    }

    #[test]
    fn test_status_serde_roundtrip() {
        let s = SetupStatus::placeholder("claude-code");
        let json = serde_json::to_string(&s).unwrap();
        let parsed: SetupStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.tool_name, s.tool_name);
    }
}

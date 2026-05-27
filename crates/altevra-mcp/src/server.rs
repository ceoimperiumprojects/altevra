use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Minimal JSON-RPC 2.0 request for MCP protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpRequest {
    pub jsonrpc: String,
    pub id: Value,
    pub method: String,
    pub params: Option<Value>,
}

/// Minimal JSON-RPC 2.0 response for MCP protocol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResponse {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<McpError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl McpResponse {
    pub fn ok(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(McpError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

/// MCP tool definitions exposed by Altevra.
pub fn list_tools() -> Value {
    serde_json::json!({
        "tools": [
            {
                "name": "get_agent_bootstrap_packet",
                "description": "Get the full bootstrap packet for an agent session start. Includes skill freshness, last updates, setup status, warnings, and recommended next action.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "tool_name": {"type": "string", "description": "Name of the tool calling bootstrap (e.g. claude-code)"},
                        "project": {"type": "string", "description": "Project identifier"},
                        "installed_skill_version": {"type": "string", "description": "Currently installed altevra-core skill version"},
                        "session_id": {"type": "string", "description": "Optional session ID for tracking"}
                    },
                    "required": ["tool_name"]
                }
            },
            {
                "name": "get_last_updates",
                "description": "Get the recent update feed items — what changed since last session or N hours.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "project": {"type": "string"},
                        "since": {"type": "string", "description": "ISO timestamp or 'last-session'"},
                        "agent_id": {"type": "string"},
                        "importance_min": {"type": "string", "enum": ["noise", "low", "medium", "high", "critical"]}
                    }
                }
            },
            {
                "name": "check_altevra_skill_version",
                "description": "Check if the installed Altevra skill is current, outdated, or missing.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "skill_slug": {"type": "string", "description": "Skill slug to check (e.g. altevra-core)"},
                        "installed_version": {"type": "string", "description": "Currently installed version"}
                    },
                    "required": ["skill_slug"]
                }
            }
        ]
    })
}

/// Route an MCP request to the correct handler.
pub struct McpServer {
    pub altevra_version: String,
    pub vault_path: std::path::PathBuf,
}

impl McpServer {
    pub fn new(altevra_version: impl Into<String>) -> Self {
        Self {
            altevra_version: altevra_version.into(),
            vault_path: std::path::PathBuf::from("."),
        }
    }

    pub fn with_vault(mut self, vault_path: impl Into<std::path::PathBuf>) -> Self {
        self.vault_path = vault_path.into();
        self
    }

    pub fn handle(&self, req: McpRequest) -> McpResponse {
        match req.method.as_str() {
            "initialize" => self.handle_initialize(req.id),
            "tools/list" => McpResponse::ok(req.id, list_tools()),
            "tools/call" => self.dispatch_tool_call(req.id, req.params),
            other => McpResponse::error(req.id, -32601, format!("Method not found: {other}")),
        }
    }

    fn handle_initialize(&self, id: Value) -> McpResponse {
        McpResponse::ok(
            id,
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": "altevra",
                    "version": self.altevra_version
                }
            }),
        )
    }

    fn dispatch_tool_call(&self, id: Value, params: Option<Value>) -> McpResponse {
        let params = params.unwrap_or_default();
        let tool_name = params["name"].as_str().unwrap_or("");
        let args = &params["arguments"];

        match tool_name {
            "get_agent_bootstrap_packet" => crate::tools_bootstrap::handle_bootstrap(
                id,
                args,
                &self.altevra_version,
                &self.vault_path,
            ),
            "get_last_updates" => crate::tools_updates::handle_get_last_updates(id, args),
            "check_altevra_skill_version" => {
                crate::tools_skills::handle_check_skill_version(id, args)
            }
            other => McpResponse::error(id, -32602, format!("Unknown tool: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tools_list_has_three_tools() {
        let server = McpServer::new("0.1.0");
        let req = McpRequest {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(1),
            method: "tools/list".to_string(),
            params: None,
        };
        let resp = server.handle(req);
        assert!(resp.error.is_none());
        let tools = &resp.result.unwrap()["tools"];
        assert_eq!(tools.as_array().unwrap().len(), 3);
    }

    #[test]
    fn test_unknown_method_returns_error() {
        let server = McpServer::new("0.1.0");
        let req = McpRequest {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(1),
            method: "nonexistent/method".to_string(),
            params: None,
        };
        let resp = server.handle(req);
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    #[test]
    fn test_response_serde_ok() {
        let resp = McpResponse::ok(serde_json::json!(42), serde_json::json!({"result": "ok"}));
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(!json.contains("error"));
    }
}

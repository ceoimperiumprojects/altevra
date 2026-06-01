use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpRequest {
    pub jsonrpc: String,
    pub id: Value,
    pub method: String,
    pub params: Option<Value>,
}

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

fn tool(name: &str, description: &str, schema: Value) -> Value {
    serde_json::json!({
        "name": name,
        "description": description,
        "inputSchema": schema,
    })
}

fn obj_schema(required: &[&str], props: Value) -> Value {
    serde_json::json!({
        "type": "object",
        "properties": props,
        "required": required,
    })
}

pub fn list_tools() -> Value {
    let mut tools = vec![];

    // Bootstrap
    tools.push(tool(
        "get_agent_bootstrap_packet",
        "Get the full bootstrap packet for an agent session start.",
        obj_schema(
            &["tool_name"],
            serde_json::json!({
                "tool_name": {"type": "string"},
                "project": {"type": "string"},
                "installed_skill_version": {"type": "string"},
                "session_id": {"type": "string"},
            }),
        ),
    ));

    // Updates
    tools.push(tool(
        "get_last_updates",
        "Get recent updates from local event feed.",
        obj_schema(
            &[],
            serde_json::json!({
                "project": {"type": "string"},
                "since": {"type": "string"},
                "agent_id": {"type": "string"},
                "importance_min": {"type": "string", "enum": ["noise", "low", "medium", "high", "critical"]},
            }),
        ),
    ));
    tools.push(tool(
        "mark_updates_read",
        "Mark updates as read for an actor.",
        obj_schema(
            &[],
            serde_json::json!({
                "actor_type": {"type": "string"},
                "actor_id": {"type": "string"},
                "last_event_id": {"type": "string"},
            }),
        ),
    ));

    // Skills
    tools.push(tool(
        "check_altevra_skill_version",
        "Check if an installed skill is current.",
        obj_schema(
            &["skill_slug"],
            serde_json::json!({
                "skill_slug": {"type": "string"},
                "installed_version": {"type": "string"},
                "vault": {"type": "string"},
            }),
        ),
    ));
    tools.push(tool(
        "get_altevra_skill",
        "Fetch a specific Altevra skill body by slug.",
        obj_schema(
            &[],
            serde_json::json!({
                "slug": {"type": "string"},
                "vault": {"type": "string"},
            }),
        ),
    ));
    tools.push(tool(
        "get_skill",
        "Alias of get_altevra_skill.",
        obj_schema(&[], serde_json::json!({"slug": {"type": "string"}})),
    ));
    tools.push(tool(
        "list_skills",
        "List all skills in the vault.",
        obj_schema(&[], serde_json::json!({"vault": {"type": "string"}})),
    ));
    tools.push(tool(
        "request_skill_refresh",
        "Request a skill to be refreshed in connected tools.",
        obj_schema(&["slug"], serde_json::json!({"slug": {"type": "string"}})),
    ));

    // Memory / context
    tools.push(tool(
        "search_memory",
        "BM25 search over vault chunks.",
        obj_schema(
            &["query"],
            serde_json::json!({
                "query": {"type": "string"},
                "vault": {"type": "string"},
                "limit": {"type": "integer"},
            }),
        ),
    ));
    tools.push(tool(
        "get_project_context",
        "Get project-scoped context: file list and sections.",
        obj_schema(
            &[],
            serde_json::json!({
                "project": {"type": "string"},
                "vault": {"type": "string"},
            }),
        ),
    ));
    tools.push(tool(
        "get_context_packet",
        "Build a full agent context packet.",
        obj_schema(
            &[],
            serde_json::json!({
                "agent": {"type": "string"},
                "vault": {"type": "string"},
            }),
        ),
    ));
    tools.push(tool(
        "get_source_of_truth",
        "Find authoritative sources for a query (decisions, skills, changes).",
        obj_schema(
            &[],
            serde_json::json!({
                "query": {"type": "string"},
                "vault": {"type": "string"},
            }),
        ),
    ));

    // Tasks / goals / decisions
    tools.push(tool(
        "get_active_tasks",
        "Get active tasks from local store.",
        obj_schema(&[], serde_json::json!({"project": {"type": "string"}})),
    ));
    tools.push(tool(
        "save_task",
        "Save a new task.",
        obj_schema(
            &["title"],
            serde_json::json!({
                "title": {"type": "string"},
                "status": {"type": "string"},
                "priority": {"type": "string"},
                "project": {"type": "string"},
            }),
        ),
    ));
    tools.push(tool(
        "update_task",
        "Update an existing task by id.",
        obj_schema(
            &["id"],
            serde_json::json!({
                "id": {"type": "string"},
                "status": {"type": "string"},
                "priority": {"type": "string"},
            }),
        ),
    ));
    tools.push(tool(
        "get_goals",
        "Get goals from local store.",
        obj_schema(&[], serde_json::json!({})),
    ));
    tools.push(tool(
        "save_decision",
        "Save a project decision.",
        obj_schema(
            &["title"],
            serde_json::json!({
                "title": {"type": "string"},
                "rationale": {"type": "string"},
                "decided_by": {"type": "string"},
            }),
        ),
    ));

    // Capabilities
    tools.push(tool(
        "get_capabilities",
        "Get the capability registry (adapters, skills, hooks).",
        obj_schema(&[], serde_json::json!({})),
    ));
    tools.push(tool(
        "report_knowledge_gap",
        "Report a knowledge gap encountered by an agent.",
        obj_schema(
            &["topic"],
            serde_json::json!({
                "topic": {"type": "string"},
                "context": {"type": "string"},
                "reporter": {"type": "string"},
            }),
        ),
    ));
    tools.push(tool(
        "report_capability_gap",
        "Report a missing capability.",
        obj_schema(
            &["capability"],
            serde_json::json!({
                "capability": {"type": "string"},
                "context": {"type": "string"},
            }),
        ),
    ));
    tools.push(tool(
        "create_review_item",
        "Create a review queue item for human attention.",
        obj_schema(
            &["title"],
            serde_json::json!({
                "kind": {"type": "string"},
                "title": {"type": "string"},
                "body": {"type": "string"},
            }),
        ),
    ));

    // Setup
    tools.push(tool(
        "get_setup_status",
        "Get tool integration status (components present, drifted, missing).",
        obj_schema(
            &[],
            serde_json::json!({
                "tool": {"type": "string"},
                "repo": {"type": "string"},
            }),
        ),
    ));
    tools.push(tool(
        "run_hook",
        "Run an Altevra hook by slug.",
        obj_schema(
            &["slug"],
            serde_json::json!({
                "slug": {"type": "string"},
                "tool": {"type": "string"},
                "project": {"type": "string"},
                "payload": {"type": "object"},
            }),
        ),
    ));

    // Prompts
    tools.push(tool(
        "build_system_prompt",
        "Assemble the layered Altevra system prompt for a given tool.",
        obj_schema(
            &["tool_name"],
            serde_json::json!({
                "tool_name": {"type": "string"},
                "project": {"type": "string"},
                "current_task": {"type": "string"},
                "current_goal": {"type": "string"},
                "vault": {"type": "string"},
                "limit": {"type": "integer"},
            }),
        ),
    ));

    // Observer
    tools.push(tool(
        "get_observer_insights",
        "Run pattern detectors over recent events and return structured insights.",
        obj_schema(
            &[],
            serde_json::json!({
                "since": {"type": "string", "description": "Window (e.g. 1h, 24h, 7d, 30d). Default 7d."},
                "write_file": {"type": "boolean", "description": "Also write vault/10-insights/auto-YYYYMMDD.md."},
            }),
        ),
    ));

    // v0.3.7.5: Research v2 — discovery + per-project search
    tools.push(tool(
        "discover_feed",
        "Scan a URL for RSS/Atom feed links and (optionally) auto-promote them.",
        obj_schema(
            &["url"],
            serde_json::json!({
                "url": {"type": "string"},
                "auto_promote": {"type": "boolean"},
            }),
        ),
    ));
    tools.push(tool(
        "github_trending",
        "Fetch GitHub Trending repositories for a language and period.",
        obj_schema(
            &[],
            serde_json::json!({
                "language": {"type": "string"},
                "since": {"type": "string", "enum": ["daily", "weekly", "monthly"]},
                "limit": {"type": "integer"},
            }),
        ),
    ));
    tools.push(tool(
        "web_search",
        "Run a web search via DuckDuckGo (default), Brave or Exa.",
        obj_schema(
            &["query"],
            serde_json::json!({
                "query": {"type": "string"},
                "provider": {"type": "string", "enum": ["ddg", "brave", "exa"]},
                "limit": {"type": "integer"},
            }),
        ),
    ));
    tools.push(tool(
        "project_research",
        "Return the project agent's keywords, queries and budget for a given project_id.",
        obj_schema(
            &["project_id"],
            serde_json::json!({
                "project_id": {"type": "string"},
                "force_run": {"type": "boolean"},
            }),
        ),
    ));

    // v0.3.7: Replay & Query
    tools.push(tool(
        "replay_session",
        "Replay a recorded session — return its full turn stream.",
        obj_schema(
            &["session_id"],
            serde_json::json!({
                "session_id": {"type": "string"},
                "turn_limit": {"type": "integer"},
                "db_path": {"type": "string"},
            }),
        ),
    ));
    tools.push(tool(
        "search_turns",
        "BM25-style search over recorded turn content.",
        obj_schema(
            &["query"],
            serde_json::json!({
                "query": {"type": "string"},
                "project": {"type": "string"},
                "tool": {"type": "string"},
                "limit": {"type": "integer"},
                "db_path": {"type": "string"},
            }),
        ),
    ));
    tools.push(tool(
        "file_history",
        "Recorded change history for a file path.",
        obj_schema(
            &["path"],
            serde_json::json!({
                "path": {"type": "string"},
                "limit": {"type": "integer"},
                "db_path": {"type": "string"},
            }),
        ),
    ));

    // v0.3 Phase 1: Wiki + Resident foundation
    tools.push(tool(
        "get_wiki_page",
        "Return the synthesized wiki page for a topic.",
        obj_schema(
            &["topic"],
            serde_json::json!({
                "topic": {"type": "string"},
                "root": {"type": "string"},
            }),
        ),
    ));
    tools.push(tool(
        "search_wiki",
        "Substring search over wiki topic / title / body.",
        obj_schema(
            &["query"],
            serde_json::json!({
                "query": {"type": "string"},
                "limit": {"type": "integer"},
                "root": {"type": "string"},
            }),
        ),
    ));
    tools.push(tool(
        "list_resident_modes",
        "List resident-agent modes available in this Altevra install.",
        obj_schema(
            &[],
            serde_json::json!({
                "root": {"type": "string"},
            }),
        ),
    ));
    tools.push(tool(
        "get_resident_prompt",
        "Return the system prompt for a resident-agent mode (`core` or a mode name).",
        obj_schema(
            &["mode"],
            serde_json::json!({
                "mode": {"type": "string"},
                "root": {"type": "string"},
            }),
        ),
    ));

    serde_json::json!({"tools": tools})
}

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
            "tools/call" => {
                // Tool handlers return bare JSON; the MCP spec (and Claude Code)
                // expect tools/call results wrapped in a `content` array, else
                // the client sees an empty result. Wrap successful results here
                // so every handler stays simple. (Found via live herdr test.)
                let id = req.id.clone();
                let resp = self.dispatch_tool_call(req.id, req.params);
                match resp.result {
                    Some(result) => {
                        let text = serde_json::to_string_pretty(&result)
                            .unwrap_or_else(|_| result.to_string());
                        McpResponse::ok(
                            id,
                            serde_json::json!({
                                "content": [{ "type": "text", "text": text }],
                                "isError": false,
                                "structuredContent": result,
                            }),
                        )
                    }
                    None => resp, // error passes through unchanged
                }
            }
            other => McpResponse::error(req.id, -32601, format!("Method not found: {other}")),
        }
    }

    fn handle_initialize(&self, id: Value) -> McpResponse {
        McpResponse::ok(
            id,
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "altevra", "version": self.altevra_version},
            }),
        )
    }

    fn dispatch_tool_call(&self, id: Value, params: Option<Value>) -> McpResponse {
        let params = params.unwrap_or_default();
        let tool_name = params["name"].as_str().unwrap_or("");
        let args = &params["arguments"];

        match tool_name {
            // Bootstrap
            "get_agent_bootstrap_packet" => crate::tools_bootstrap::handle_bootstrap(
                id,
                args,
                &self.altevra_version,
                &self.vault_path,
            ),
            // Updates
            "get_last_updates" => crate::tools_updates::handle_get_last_updates(id, args),
            "mark_updates_read" => crate::tools_updates::handle_mark_updates_read(id, args),
            // Skills
            "check_altevra_skill_version" => {
                crate::tools_skills::handle_check_skill_version(id, args)
            }
            "get_altevra_skill" => crate::tools_skills::handle_get_altevra_skill(id, args),
            "get_skill" => crate::tools_skills::handle_get_skill(id, args),
            "list_skills" => crate::tools_skills::handle_list_skills(id, args),
            "request_skill_refresh" => crate::tools_skills::handle_request_skill_refresh(id, args),
            // Memory / context
            "search_memory" => crate::tools_memory::handle_search_memory(id, args),
            "get_project_context" => crate::tools_memory::handle_get_project_context(id, args),
            "get_context_packet" => crate::tools_memory::handle_get_context_packet(id, args),
            "get_source_of_truth" => crate::tools_memory::handle_get_source_of_truth(id, args),
            // Tasks
            "get_active_tasks" => crate::tools_tasks::handle_get_active_tasks(id, args),
            "save_task" => crate::tools_tasks::handle_save_task(id, args),
            "update_task" => crate::tools_tasks::handle_update_task(id, args),
            "get_goals" => crate::tools_tasks::handle_get_goals(id, args),
            "save_decision" => crate::tools_tasks::handle_save_decision(id, args),
            // Capabilities
            "get_capabilities" => crate::tools_capabilities::handle_get_capabilities(id, args),
            "report_knowledge_gap" => {
                crate::tools_capabilities::handle_report_knowledge_gap(id, args)
            }
            "report_capability_gap" => {
                crate::tools_capabilities::handle_report_capability_gap(id, args)
            }
            "create_review_item" => crate::tools_capabilities::handle_create_review_item(id, args),
            // Setup
            "get_setup_status" => crate::tools_setup::handle_get_setup_status(id, args),
            "run_hook" => crate::tools_setup::handle_run_hook(id, args),
            // Prompts
            "build_system_prompt" => {
                crate::tools_prompts::handle_build_system_prompt(id, args, &self.altevra_version)
            }
            // Observer
            "get_observer_insights" => {
                crate::tools_observer::handle_get_observer_insights(id, args, &self.vault_path)
            }
            // v0.3.7: Replay & Query
            "replay_session" => crate::tools_sessions::handle_replay_session(id, args),
            "search_turns" => crate::tools_sessions::handle_search_turns(id, args),
            "file_history" => crate::tools_sessions::handle_file_history(id, args),
            // v0.3.7.5: Research v2
            "discover_feed" => crate::tools_discovery::handle_discover_feed(id, args),
            "github_trending" => crate::tools_discovery::handle_github_trending(id, args),
            "web_search" => crate::tools_discovery::handle_web_search(id, args),
            "project_research" => crate::tools_discovery::handle_project_research(id, args),
            // v0.3 Phase 1: Wiki + Resident
            "get_wiki_page" => crate::tools_wiki::handle_get_wiki_page(id, args),
            "search_wiki" => crate::tools_wiki::handle_search_wiki(id, args),
            "list_resident_modes" => crate::tools_wiki::handle_list_resident_modes(id, args),
            "get_resident_prompt" => crate::tools_wiki::handle_get_resident_prompt(id, args),
            other => McpResponse::error(id, -32602, format!("Unknown tool: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tools_list_has_all_tools() {
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
        let count = tools.as_array().unwrap().len();
        assert!(
            count >= 22,
            "expected ≥22 tools (architecture v5 list), got {count}"
        );
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
    }

    #[test]
    fn test_dispatch_unknown_tool_errors() {
        let server = McpServer::new("0.1.0");
        let req = McpRequest {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(1),
            method: "tools/call".to_string(),
            params: Some(serde_json::json!({"name": "nonexistent", "arguments": {}})),
        };
        let resp = server.handle(req);
        assert!(resp.error.is_some());
    }

    #[test]
    fn test_response_serde_ok() {
        let resp = McpResponse::ok(serde_json::json!(42), serde_json::json!({"result": "ok"}));
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(!json.contains("error"));
    }
}

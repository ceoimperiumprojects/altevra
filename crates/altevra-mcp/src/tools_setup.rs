use serde_json::Value;

use crate::server::McpResponse;

pub fn handle_get_setup_status(id: Value, args: &Value) -> McpResponse {
    let tool = args["tool"].as_str().unwrap_or("claude-code");
    let repo = args["repo"].as_str().unwrap_or(".");
    let repo_path = std::path::Path::new(repo);

    let vault_ok = repo_path.join(".altevra/config.toml").exists();
    let skills_ok = repo_path.join("06-skills").is_dir();

    let mut tool_components = serde_json::Map::new();
    match tool {
        "claude-code" => {
            tool_components.insert(
                "instructions".into(),
                serde_json::json!(repo_path.join(".claude/altevra-instructions.md").exists()),
            );
            tool_components.insert(
                "settings".into(),
                serde_json::json!(repo_path.join(".claude/settings.json").exists()),
            );
            tool_components.insert(
                "skills_dir".into(),
                serde_json::json!(repo_path.join(".claude/skills").is_dir()),
            );
        }
        "codex" => {
            tool_components.insert(
                "agents_md".into(),
                serde_json::json!(repo_path.join("AGENTS.md").exists()),
            );
            tool_components.insert(
                "config".into(),
                serde_json::json!(repo_path.join(".codex/config.toml").exists()),
            );
        }
        "cursor" => {
            tool_components.insert(
                "rules".into(),
                serde_json::json!(repo_path.join(".cursor/rules/altevra.mdc").exists()),
            );
            tool_components.insert(
                "mcp".into(),
                serde_json::json!(repo_path.join(".cursor/mcp.json").exists()),
            );
        }
        "antigravity" => {
            tool_components.insert(
                "agents_md".into(),
                serde_json::json!(repo_path.join("AGENTS.md").exists()),
            );
            tool_components.insert(
                "mcp".into(),
                serde_json::json!(repo_path.join(".gemini/config/mcp_config.json").exists()),
            );
            tool_components.insert(
                "hooks".into(),
                serde_json::json!(repo_path.join(".agent/hooks/altevra_hooks.py").exists()),
            );
        }
        _ => {}
    }

    McpResponse::ok(
        id,
        serde_json::json!({
            "tool": tool,
            "vault_initialized": vault_ok,
            "skills_dir": skills_ok,
            "components": tool_components,
        }),
    )
}

pub fn handle_run_hook(id: Value, args: &Value) -> McpResponse {
    use altevra_hooks::{HookRegistry, HookRunContext, HookRunner};

    let slug = match args["slug"].as_str() {
        Some(s) => s.to_string(),
        None => return McpResponse::error(id, -32602, "missing 'slug'"),
    };
    let tool = args["tool"].as_str().unwrap_or("unknown").to_string();
    let registry = HookRegistry::with_defaults();
    let runner = HookRunner::new(&registry);
    let ctx = HookRunContext {
        hook_slug: slug.clone(),
        tool_name: tool,
        project: args["project"].as_str().map(String::from),
        session_id: args["session_id"].as_str().map(String::from),
        payload: args["payload"].clone(),
    };
    let outcome = runner.run(ctx);
    match serde_json::to_value(&outcome) {
        Ok(v) => McpResponse::ok(id, v),
        Err(e) => McpResponse::error(id, -32000, format!("serialize failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_hook_missing_slug_errors() {
        let resp = handle_run_hook(Value::from(1), &serde_json::json!({}));
        assert!(resp.error.is_some());
    }

    #[test]
    fn setup_status_returns_components() {
        let tmp = tempfile::TempDir::new().unwrap();
        let resp = handle_get_setup_status(
            Value::from(1),
            &serde_json::json!({"tool": "claude-code", "repo": tmp.path().to_string_lossy()}),
        );
        assert!(resp.error.is_none());
        let v = resp.result.unwrap();
        assert!(v["components"].is_object());
    }
}

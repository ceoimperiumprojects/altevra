use serde_json::Value;

use crate::server::McpResponse;

const CAP_PATH: &str = ".altevra/state/capabilities.json";
const KGAP_PATH: &str = ".altevra/state/knowledge_gaps.jsonl";
const CGAP_PATH: &str = ".altevra/state/capability_gaps.jsonl";
const REVIEW_PATH: &str = ".altevra/state/review_items.jsonl";

fn append_jsonl(path: &str, value: &Value) -> std::io::Result<()> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{}", serde_json::to_string(value)?)?;
    Ok(())
}

#[allow(dead_code)]
fn load_jsonl(path: &str) -> Vec<Value> {
    if !std::path::Path::new(path).exists() {
        return vec![];
    }
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

pub fn handle_get_capabilities(id: Value, _args: &Value) -> McpResponse {
    let caps = if std::path::Path::new(CAP_PATH).exists() {
        std::fs::read_to_string(CAP_PATH)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| serde_json::json!({"adapters": [], "skills": [], "hooks": []}))
    } else {
        // Defaults
        serde_json::json!({
            "adapters": ["claude-code", "codex", "cursor", "antigravity"],
            "skills": [],
            "hooks": ["session_start", "session_end", "on_error"],
            "mcp_tools": 22,
            "cli_commands": 15,
        })
    };
    McpResponse::ok(id, caps)
}

pub fn handle_report_knowledge_gap(id: Value, args: &Value) -> McpResponse {
    let topic = match args["topic"].as_str() {
        Some(t) => t.to_string(),
        None => return McpResponse::error(id, -32602, "missing 'topic'"),
    };
    let entry = serde_json::json!({
        "id": uuid::Uuid::new_v4(),
        "topic": topic,
        "context": args["context"],
        "reported_at": chrono::Utc::now(),
        "reporter": args["reporter"],
    });
    if let Err(e) = append_jsonl(KGAP_PATH, &entry) {
        return McpResponse::error(id, -32000, format!("save failed: {e}"));
    }
    McpResponse::ok(id, entry)
}

pub fn handle_report_capability_gap(id: Value, args: &Value) -> McpResponse {
    let capability = match args["capability"].as_str() {
        Some(c) => c.to_string(),
        None => return McpResponse::error(id, -32602, "missing 'capability'"),
    };
    let entry = serde_json::json!({
        "id": uuid::Uuid::new_v4(),
        "capability": capability,
        "context": args["context"],
        "reported_at": chrono::Utc::now(),
    });
    if let Err(e) = append_jsonl(CGAP_PATH, &entry) {
        return McpResponse::error(id, -32000, format!("save failed: {e}"));
    }
    McpResponse::ok(id, entry)
}

pub fn handle_create_review_item(id: Value, args: &Value) -> McpResponse {
    let title = match args["title"].as_str() {
        Some(t) => t.to_string(),
        None => return McpResponse::error(id, -32602, "missing 'title'"),
    };
    let entry = serde_json::json!({
        "id": uuid::Uuid::new_v4(),
        "kind": args["kind"].as_str().unwrap_or("note"),
        "title": title,
        "body": args["body"],
        "status": "open",
        "created_at": chrono::Utc::now(),
    });
    if let Err(e) = append_jsonl(REVIEW_PATH, &entry) {
        return McpResponse::error(id, -32000, format!("save failed: {e}"));
    }
    McpResponse::ok(id, entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_knowledge_gap_missing_topic_errors() {
        let resp = handle_report_knowledge_gap(Value::from(1), &serde_json::json!({}));
        assert!(resp.error.is_some());
    }

    #[test]
    fn jsonl_load_empty() {
        let v = load_jsonl("/tmp/this-should-not-exist-altevra.jsonl");
        assert!(v.is_empty());
    }
}

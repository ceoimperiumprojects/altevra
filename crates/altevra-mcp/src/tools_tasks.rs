use serde_json::Value;

use crate::server::McpResponse;

/// In-memory task store using a local JSON file at .altevra/state/tasks.json.
/// Real DB-backed implementation lives in altevra-db.

const STATE_PATH: &str = ".altevra/state/tasks.json";
const DEC_PATH: &str = ".altevra/state/decisions.json";
const GOAL_PATH: &str = ".altevra/state/goals.json";

fn load_json(path: &str) -> Value {
    if std::path::Path::new(path).exists() {
        let raw = std::fs::read_to_string(path).unwrap_or_default();
        serde_json::from_str(&raw).unwrap_or_else(|_| serde_json::json!([]))
    } else {
        serde_json::json!([])
    }
}

fn save_json(path: &str, value: &Value) -> std::io::Result<()> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(value)?)?;
    Ok(())
}

pub fn handle_get_active_tasks(id: Value, _args: &Value) -> McpResponse {
    let tasks = load_json(STATE_PATH);
    let active: Vec<&Value> = tasks
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter(|t| {
                    t["status"]
                        .as_str()
                        .map(|s| s != "completed" && s != "cancelled")
                        .unwrap_or(true)
                })
                .collect()
        })
        .unwrap_or_default();
    McpResponse::ok(
        id,
        serde_json::json!({"tasks": active, "count": active.len()}),
    )
}

pub fn handle_save_task(id: Value, args: &Value) -> McpResponse {
    let title = match args["title"].as_str() {
        Some(t) => t.to_string(),
        None => return McpResponse::error(id, -32602, "missing 'title'"),
    };
    let mut tasks = load_json(STATE_PATH);
    let new_task = serde_json::json!({
        "id": uuid::Uuid::new_v4(),
        "title": title,
        "status": args["status"].as_str().unwrap_or("open"),
        "priority": args["priority"].as_str().unwrap_or("medium"),
        "project": args["project"],
        "created_at": chrono::Utc::now(),
    });
    if let Some(arr) = tasks.as_array_mut() {
        arr.push(new_task.clone());
    }
    if let Err(e) = save_json(STATE_PATH, &tasks) {
        return McpResponse::error(id, -32000, format!("save failed: {e}"));
    }
    McpResponse::ok(id, new_task)
}

pub fn handle_update_task(id: Value, args: &Value) -> McpResponse {
    let task_id = match args["id"].as_str() {
        Some(t) => t.to_string(),
        None => return McpResponse::error(id, -32602, "missing 'id'"),
    };
    let mut tasks = load_json(STATE_PATH);
    let arr = match tasks.as_array_mut() {
        Some(a) => a,
        None => return McpResponse::error(id, -32000, "tasks store malformed"),
    };
    let mut updated = false;
    for t in arr.iter_mut() {
        if t["id"].as_str() == Some(&task_id) {
            if let Some(status) = args["status"].as_str() {
                t["status"] = serde_json::json!(status);
            }
            if let Some(prio) = args["priority"].as_str() {
                t["priority"] = serde_json::json!(prio);
            }
            t["updated_at"] = serde_json::json!(chrono::Utc::now());
            updated = true;
        }
    }
    if !updated {
        return McpResponse::error(id, -32000, format!("Task not found: {task_id}"));
    }
    let _ = save_json(STATE_PATH, &tasks);
    McpResponse::ok(id, serde_json::json!({"updated": true, "id": task_id}))
}

pub fn handle_get_goals(id: Value, _args: &Value) -> McpResponse {
    let goals = load_json(GOAL_PATH);
    let count = goals.as_array().map(|a| a.len()).unwrap_or(0);
    McpResponse::ok(id, serde_json::json!({"goals": goals, "count": count}))
}

pub fn handle_save_decision(id: Value, args: &Value) -> McpResponse {
    let title = match args["title"].as_str() {
        Some(t) => t.to_string(),
        None => return McpResponse::error(id, -32602, "missing 'title'"),
    };
    let mut decisions = load_json(DEC_PATH);
    let new = serde_json::json!({
        "id": uuid::Uuid::new_v4(),
        "title": title,
        "rationale": args["rationale"],
        "decided_at": chrono::Utc::now(),
        "decided_by": args["decided_by"],
    });
    if let Some(arr) = decisions.as_array_mut() {
        arr.push(new.clone());
    }
    let _ = save_json(DEC_PATH, &decisions);
    McpResponse::ok(id, new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_task_missing_title_errors() {
        let resp = handle_save_task(Value::from(1), &serde_json::json!({}));
        assert!(resp.error.is_some());
    }

    #[test]
    fn get_active_tasks_returns_structure() {
        let resp = handle_get_active_tasks(Value::from(1), &serde_json::json!({}));
        assert!(resp.error.is_none());
    }
}

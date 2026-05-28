use altevra_core::updates::{Importance, UpdateFeedItem};
use serde_json::Value;
use uuid::Uuid;

use crate::server::McpResponse;

pub fn handle_get_last_updates(id: Value, args: &Value) -> McpResponse {
    let since = args["since"]
        .as_str()
        .map(parse_since_str)
        .unwrap_or_else(|| chrono::Utc::now() - chrono::Duration::hours(24));
    let project_filter = args["project"].as_str();
    let importance_min = args["importance_min"]
        .as_str()
        .and_then(|s| s.parse::<Importance>().ok());

    let items = load_local(since, project_filter, importance_min.as_ref());

    McpResponse::ok(
        id,
        serde_json::json!({
            "updates": items,
            "count": items.len(),
            "query": {
                "since": args["since"],
                "project": args["project"],
                "importance_min": args["importance_min"],
            },
        }),
    )
}

pub fn handle_mark_updates_read(id: Value, args: &Value) -> McpResponse {
    let actor_type = args["actor_type"].as_str().unwrap_or("agent");
    let actor_id = args["actor_id"].as_str().unwrap_or("default");
    let last_event = args["last_event_id"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok());

    let path = std::path::Path::new(".altevra/state/read_state.json");
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return McpResponse::error(id, -32000, format!("mkdir failed: {e}"));
        }
    }
    let mut map: serde_json::Map<String, Value> = if path.exists() {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        Default::default()
    };
    let key = format!("{actor_type}::{actor_id}");
    map.insert(
        key,
        serde_json::json!({
            "last_seen_event_id": last_event,
            "last_seen_at": chrono::Utc::now(),
        }),
    );
    if let Err(e) = std::fs::write(path, serde_json::to_string_pretty(&map).unwrap_or_default()) {
        return McpResponse::error(id, -32000, format!("write failed: {e}"));
    }
    McpResponse::ok(id, serde_json::json!({"marked_read": true}))
}

fn load_local(
    since: chrono::DateTime<chrono::Utc>,
    project: Option<&str>,
    importance_min: Option<&Importance>,
) -> Vec<UpdateFeedItem> {
    let path = std::path::Path::new(".altevra/events/updates.jsonl");
    if !path.exists() {
        return vec![];
    }
    let content = std::fs::read_to_string(path).unwrap_or_default();
    let mut items: Vec<UpdateFeedItem> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .filter(|u: &UpdateFeedItem| u.created_at >= since)
        .filter(|u: &UpdateFeedItem| {
            project
                .map(|p| u.title.contains(p) || u.update_type.contains(p))
                .unwrap_or(true)
        })
        .filter(|u: &UpdateFeedItem| {
            importance_min
                .map(|imin| &u.importance >= imin)
                .unwrap_or(true)
        })
        .collect();
    items.sort_by_key(|i| std::cmp::Reverse(i.created_at));
    items
}

fn parse_since_str(s: &str) -> chrono::DateTime<chrono::Utc> {
    use chrono::Utc;
    match s {
        "last-session" => Utc::now() - chrono::Duration::hours(24),
        "1h" | "last-hour" => Utc::now() - chrono::Duration::hours(1),
        "24h" => Utc::now() - chrono::Duration::hours(24),
        "7d" => Utc::now() - chrono::Duration::days(7),
        other => other
            .parse::<chrono::DateTime<Utc>>()
            .unwrap_or_else(|_| Utc::now() - chrono::Duration::hours(24)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_last_updates_returns_structure() {
        let id = serde_json::json!(1);
        let args = serde_json::json!({"project": "altevra", "since": "24h"});
        let resp = handle_get_last_updates(id, &args);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert!(result["updates"].is_array());
        assert!(result["count"].is_number());
    }
}

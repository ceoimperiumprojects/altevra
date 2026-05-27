use altevra_core::events::{ActorType, Event, EventStatus, EventType};
use altevra_core::observer::{detect_patterns, writer};
use altevra_core::security::Sensitivity;
use altevra_core::updates::UpdateFeedItem;
use chrono::{DateTime, Duration, Utc};
use serde_json::Value;
use std::path::Path;
use std::str::FromStr;

use crate::server::McpResponse;

pub fn handle_get_observer_insights(id: Value, args: &Value, vault_root: &Path) -> McpResponse {
    let since_str = args["since"].as_str().unwrap_or("7d").to_string();
    let write_file = args["write_file"].as_bool().unwrap_or(false);

    let since = Utc::now() - parse_window(&since_str);
    let events = load_events_for_observer(vault_root, since);
    let updates = load_updates(vault_root, since);

    let insights = detect_patterns(&events, &updates);

    let written_path = if write_file {
        match writer::write_insights_markdown(&insights, vault_root) {
            Ok(p) => Some(p.display().to_string()),
            Err(e) => {
                return McpResponse::error(id, -32000, format!("write failed: {e}"));
            }
        }
    } else {
        None
    };

    McpResponse::ok(
        id,
        serde_json::json!({
            "since": since_str,
            "count": insights.len(),
            "insights": insights,
            "written_path": written_path,
        }),
    )
}

fn parse_window(s: &str) -> Duration {
    match s {
        "1h" => Duration::hours(1),
        "24h" | "1d" => Duration::hours(24),
        "7d" => Duration::days(7),
        "14d" => Duration::days(14),
        "30d" => Duration::days(30),
        other => other
            .strip_suffix('h')
            .and_then(|n| n.parse::<i64>().ok())
            .map(Duration::hours)
            .or_else(|| {
                other
                    .strip_suffix('d')
                    .and_then(|n| n.parse::<i64>().ok())
                    .map(Duration::days)
            })
            .unwrap_or_else(|| Duration::hours(24)),
    }
}

fn load_updates(vault: &Path, since: DateTime<Utc>) -> Vec<UpdateFeedItem> {
    let path = vault.join(".altevra/events/updates.jsonl");
    if !path.exists() {
        return vec![];
    }
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<UpdateFeedItem>(l).ok())
        .filter(|u| u.created_at >= since)
        .collect()
}

fn load_events_for_observer(vault: &Path, since: DateTime<Utc>) -> Vec<Event> {
    let events_path = vault.join(".altevra/events/events.jsonl");
    if events_path.exists() {
        let content = std::fs::read_to_string(&events_path).unwrap_or_default();
        return content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str::<Event>(l).ok())
            .filter(|e| e.created_at >= since)
            .collect();
    }
    // Fallback: derive minimal Events from updates.
    let updates = load_updates(vault, since);
    updates
        .into_iter()
        .filter_map(|u| {
            let et = EventType::from_str(&u.update_type).ok()?;
            Some(Event {
                id: u.event_id,
                event_type: et,
                project_id: u.project_id,
                actor_type: ActorType::System,
                actor_id: None,
                source: u.update_type.clone(),
                entity_type: None,
                entity_id: extract_entity_id(&u.affected_entities),
                title: u.title,
                summary: Some(u.short_summary),
                payload: serde_json::Value::Object(Default::default()),
                sensitivity: Sensitivity::Internal,
                created_at: u.created_at,
                processed_at: None,
                status: EventStatus::Processed,
            })
        })
        .collect()
}

fn extract_entity_id(v: &Value) -> Option<String> {
    v.as_array()
        .and_then(|arr| arr.first())
        .and_then(|first| first.get("id"))
        .and_then(|id| id.as_str())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use altevra_core::updates::Importance;
    use uuid::Uuid;

    #[test]
    fn handles_empty_vault() {
        let tmp = tempfile::tempdir().unwrap();
        let id = serde_json::json!(1);
        let args = serde_json::json!({"since": "7d"});
        let resp = handle_get_observer_insights(id, &args, tmp.path());
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["count"], 0);
        assert!(result["insights"].is_array());
        assert!(result["written_path"].is_null());
    }

    #[test]
    fn detects_drift_from_synthesized_events_and_writes_file() {
        let tmp = tempfile::tempdir().unwrap();
        let events_dir = tmp.path().join(".altevra/events");
        std::fs::create_dir_all(&events_dir).unwrap();

        // Three updates of skill_drift_detected, same entity_id "foo".
        let mut lines = String::new();
        for _ in 0..3 {
            let u = UpdateFeedItem {
                id: Uuid::new_v4(),
                event_id: Uuid::new_v4(),
                project_id: None,
                update_type: "skill_drift_detected".to_string(),
                importance: Importance::High,
                title: "drift foo".to_string(),
                short_summary: "drift".to_string(),
                agent_summary: None,
                affected_entities: serde_json::json!([{"type": "skill", "id": "foo"}]),
                recommended_agent_action: None,
                visible_to_agents: true,
                sensitivity: Sensitivity::Internal,
                created_at: Utc::now() - Duration::hours(1),
            };
            lines.push_str(&serde_json::to_string(&u).unwrap());
            lines.push('\n');
        }
        std::fs::write(events_dir.join("updates.jsonl"), lines).unwrap();

        let args = serde_json::json!({"since": "7d", "write_file": true});
        let resp = handle_get_observer_insights(serde_json::json!(2), &args, tmp.path());
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert!(result["count"].as_u64().unwrap() >= 1);
        assert!(result["insights"].is_array());
        // File should exist.
        let written = result["written_path"].as_str().unwrap();
        assert!(std::path::Path::new(written).exists());
    }

    #[test]
    fn parse_window_supports_dynamic_units() {
        assert_eq!(parse_window("48h"), Duration::hours(48));
        assert_eq!(parse_window("10d"), Duration::days(10));
        assert_eq!(parse_window("garbage"), Duration::hours(24));
    }
}

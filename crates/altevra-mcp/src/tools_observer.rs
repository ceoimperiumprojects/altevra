use altevra_core::observer::{detect_patterns, writer};
use altevra_core::updates::UpdateFeedItem;
use chrono::{DateTime, Duration, Utc};
use serde_json::Value;
use std::path::Path;

use crate::server::McpResponse;

pub fn handle_get_observer_insights(id: Value, args: &Value, vault_root: &Path) -> McpResponse {
    let since_str = args["since"].as_str().unwrap_or("7d").to_string();
    let write_file = args["write_file"].as_bool().unwrap_or(false);
    // Optional explicit db_path; falls back to default_db_path().
    let db_path = args
        .get("db_path")
        .and_then(|v| v.as_str())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(altevra_core::default_db_path);

    let since = Utc::now() - parse_window(&since_str);

    // Query SQLite first; fall back to flat JSONL if unreachable or empty.
    let (events, updates) = load_events_from_db_sync(&db_path, vault_root, since);

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

/// Load events from SQLite (primary) with flat-JSONL fallback.
///
/// Runs synchronously via a fresh single-threaded tokio runtime so this can be
/// called from the MCP server's sync handler context.
fn load_events_from_db_sync(
    db_path: &std::path::Path,
    vault: &Path,
    since: DateTime<Utc>,
) -> (Vec<altevra_core::events::Event>, Vec<UpdateFeedItem>) {
    // Attempt SQLite via a temporary single-thread runtime.
    if db_path.exists() {
        let result = std::thread::spawn({
            let db_path = db_path.to_path_buf();
            let since = since;
            move || {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .ok()?
                    .block_on(async {
                        let pool =
                            altevra_db::create_pool(&db_path.to_string_lossy()).await.ok()?;
                        let _ = altevra_db::run_migrations(&pool).await;
                        let events = altevra_db::EventsRepository::new(&pool)
                            .list_since(since, None, 5000)
                            .await
                            .ok()?;
                        Some(events)
                    })
            }
        })
        .join()
        .ok()
        .flatten();

        if let Some(events) = result {
            if !events.is_empty() {
                return (events, vec![]);
            }
        }
    }

    // Fallback: flat JSONL (dev/test/pre-hook environments).
    load_events_from_jsonl(vault, since)
}

/// Flat-JSONL loader — kept for dev/test use and backwards compat.
fn load_events_from_jsonl(
    vault: &Path,
    since: DateTime<Utc>,
) -> (Vec<altevra_core::events::Event>, Vec<UpdateFeedItem>) {
    use altevra_core::events::{ActorType, Event, EventStatus, EventType};
    use altevra_core::security::Sensitivity;
    use std::str::FromStr;

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

    fn extract_entity_id(v: &Value) -> Option<String> {
        v.as_array()
            .and_then(|arr| arr.first())
            .and_then(|first| first.get("id"))
            .and_then(|id| id.as_str())
            .map(|s| s.to_string())
    }

    let events_path = vault.join(".altevra/events/events.jsonl");
    if events_path.exists() {
        let content = std::fs::read_to_string(&events_path).unwrap_or_default();
        let events: Vec<Event> = content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str::<Event>(l).ok())
            .filter(|e| e.created_at >= since)
            .collect();
        return (events, vec![]);
    }

    // Last-resort: synthesize Events from updates.jsonl stream.
    let updates = load_updates(vault, since);
    let events: Vec<Event> = updates
        .iter()
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
                title: u.title.clone(),
                summary: Some(u.short_summary.clone()),
                payload: serde_json::Value::Object(Default::default()),
                sensitivity: Sensitivity::Internal,
                created_at: u.created_at,
                processed_at: None,
                status: EventStatus::Processed,
            })
        })
        .collect();
    (events, updates)
}

#[cfg(test)]
mod tests {
    use super::*;
    use altevra_core::security::Sensitivity;
    use altevra_core::updates::Importance;
    use uuid::Uuid;

    #[test]
    fn handles_empty_vault() {
        let tmp = tempfile::tempdir().unwrap();
        let id = serde_json::json!(1);
        // Point db_path at a non-existent file → JSONL fallback → empty → count 0.
        let args = serde_json::json!({
            "since": "7d",
            "db_path": tmp.path().join("nonexistent.db").to_str().unwrap(),
        });
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

        // Point db_path at nonexistent → falls back to JSONL (updates.jsonl seeded above).
        let args = serde_json::json!({
            "since": "7d",
            "write_file": true,
            "db_path": tmp.path().join("nonexistent.db").to_str().unwrap(),
        });
        let resp = handle_get_observer_insights(serde_json::json!(2), &args, tmp.path());
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert!(result["count"].as_u64().unwrap() >= 1);
        assert!(result["insights"].is_array());
        // File should exist.
        let written = result["written_path"].as_str().unwrap();
        assert!(std::path::Path::new(written).exists());
    }

    /// Fixture test: seed SQLite events → MCP observer returns >=1 insight (via db_path arg).
    #[test]
    fn mcp_observer_returns_insight_from_seeded_sqlite_events() {
        use altevra_core::events::{ActorType, Event, EventType};
        use altevra_db::{EventsRepository, create_pool, run_migrations};

        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test.db");

        // Seed 3 SkillDriftDetected events → RecurringDrift insight.
        let pool = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let pool = create_pool(&db_path.to_string_lossy()).await.unwrap();
                run_migrations(&pool).await.unwrap();
                let repo = EventsRepository::new(&pool);
                for h in [2i64, 4, 6] {
                    let mut ev = Event::new(
                        EventType::SkillDriftDetected,
                        "drift altevra-core",
                        "test",
                        ActorType::System,
                    )
                    .with_entity("skill", "altevra-core");
                    ev.created_at = Utc::now() - Duration::hours(h);
                    repo.insert(&ev).await.unwrap();
                }
                pool
            });
        drop(pool); // release connections before sync handler spawns its own

        let args = serde_json::json!({
            "since": "7d",
            "db_path": db_path.to_str().unwrap(),
        });
        let resp =
            handle_get_observer_insights(serde_json::json!(3), &args, tmp.path());
        assert!(resp.error.is_none(), "observer returned error: {:?}", resp.error);
        let result = resp.result.unwrap();
        let count = result["count"].as_u64().unwrap_or(0);
        assert!(
            count >= 1,
            "expected >=1 insight from seeded SQLite events, got {count}"
        );
    }

    #[test]
    fn parse_window_supports_dynamic_units() {
        assert_eq!(parse_window("48h"), Duration::hours(48));
        assert_eq!(parse_window("10d"), Duration::days(10));
        assert_eq!(parse_window("garbage"), Duration::hours(24));
    }
}

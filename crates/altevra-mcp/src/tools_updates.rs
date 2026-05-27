use altevra_core::updates::Importance;
use serde_json::Value;
use uuid::Uuid;

use crate::server::McpResponse;

pub fn handle_get_last_updates(id: Value, args: &Value) -> McpResponse {
    let _since = args["since"].as_str().map(parse_since_str);
    let _project_id: Option<Uuid> = None; // TODO: resolve project name to UUID from DB
    let _importance_min = args["importance_min"]
        .as_str()
        .and_then(|s| s.parse::<Importance>().ok());

    // In MVP: return empty list with metadata (DB not available without connection)
    let result = serde_json::json!({
        "updates": [],
        "count": 0,
        "query": {
            "since": args["since"],
            "project": args["project"],
            "importance_min": args["importance_min"],
        },
        "note": "Connect to Altevra database for live updates. Run: altevra serve"
    });

    McpResponse::ok(id, result)
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
        let args = serde_json::json!({
            "project": "altevra",
            "since": "24h"
        });
        let resp = handle_get_last_updates(id, &args);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert!(result["updates"].is_array());
        assert!(result["count"].is_number());
    }
}

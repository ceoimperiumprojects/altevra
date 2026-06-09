//! MCP tools for v0.3.7 Replay & Query: replay_session, search_turns, file_history.

use crate::server::McpResponse;
use altevra_db::TurnRow;
use serde_json::Value;
use std::path::PathBuf;

/// R11 #4: turn reads (replay/search) are an exposure surface — they must pass
/// through the ExposureGate, not return raw content. An MCP/agent caller gets a
/// work ceiling: a turn classified Restricted (health/personal) or not
/// sufficiently redacted (unscanned) is denied. Turns carry no domain, so we
/// stamp Business (in the work scope) and let sensitivity + redaction decide —
/// personal content is excluded by its Restricted level, never by guesswork.
fn turn_exposable(t: &TurnRow) -> bool {
    use altevra_core::envelope::{Envelope, Provenance, ProvenanceOrigin};
    use altevra_core::safety::{ExposureGate, ExposureRequest};
    use altevra_core::security::Sensitivity;
    use altevra_core::status::RedactionStatus;

    let mut env = Envelope::new(
        t.id.to_string(),
        "turn",
        t.created_at,
        Provenance::new(ProvenanceOrigin::Imported),
    );
    // unknown sensitivity → Other → ranks max (fail-closed).
    env.sensitivity = t.sensitivity.parse::<Sensitivity>().unwrap();
    env.domain = altevra_core::domain::Domain::Business;
    let redaction = t
        .redaction_status
        .parse::<RedactionStatus>()
        .unwrap_or(RedactionStatus::Unscanned);
    ExposureGate::decide(&env, &redaction, &ExposureRequest::default_work()).is_allowed()
}

fn db_path_from_args(args: &Value) -> PathBuf {
    args.get("db_path")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .unwrap_or_else(altevra_core::default_db_path)
}

async fn open_pool(db_path: &std::path::Path) -> anyhow::Result<sqlx::SqlitePool> {
    let pool = altevra_db::create_pool(&db_path.to_string_lossy()).await?;
    altevra_db::run_migrations(&pool).await?;
    Ok(pool)
}

pub fn handle_replay_session(id: Value, args: &Value) -> McpResponse {
    let session_id = args
        .get("session_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let turn_limit = args
        .get("turn_limit")
        .and_then(|v| v.as_i64())
        .unwrap_or(1000);
    let db_path = db_path_from_args(args);

    if session_id.is_empty() {
        return McpResponse::error(id, -32602, "session_id required");
    }
    let session_uuid = match uuid::Uuid::parse_str(session_id) {
        Ok(u) => u,
        Err(e) => return McpResponse::error(id, -32602, format!("invalid uuid: {e}")),
    };

    let rt = match tokio::runtime::Handle::try_current() {
        Ok(h) => h,
        Err(_) => return McpResponse::error(id, -32603, "no tokio runtime"),
    };

    let result: anyhow::Result<Value> = futures::executor::block_on(async {
        let pool = open_pool(&db_path).await?;
        let repo = altevra_db::SessionsRepository::new(&pool);
        let session = repo
            .get_session(session_uuid)
            .await?
            .ok_or_else(|| anyhow::anyhow!("session not found"))?;
        let all_turns = repo.list_turns(session_uuid, turn_limit).await?;
        let total = all_turns.len();
        let turns: Vec<_> = all_turns.into_iter().filter(turn_exposable).collect();
        let gated = total - turns.len();
        Ok(serde_json::json!({
            "session": {
                "id": session.id,
                "tool": session.tool,
                "project": session.project_name,
                "started_at": session.started_at,
                "ended_at": session.ended_at,
                "turn_count": session.turn_count,
            },
            "turns": turns.iter().map(|t| serde_json::json!({
                "idx": t.turn_idx,
                "role": t.role,
                "content": t.content,
                "tool_name": t.tool_name,
                "created_at": t.created_at,
            })).collect::<Vec<_>>(),
            // transparency: how many turns the exposure gate withheld (no detail).
            "gated_turns": gated,
        }))
    });
    let _ = rt; // silence unused

    match result {
        Ok(v) => McpResponse::ok(id, v),
        Err(e) => McpResponse::error(id, -32603, e.to_string()),
    }
}

pub fn handle_search_turns(id: Value, args: &Value) -> McpResponse {
    let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
    let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(10);
    let project = args.get("project").and_then(|v| v.as_str());
    let tool = args.get("tool").and_then(|v| v.as_str());
    let db_path = db_path_from_args(args);

    if query.is_empty() {
        return McpResponse::error(id, -32602, "query required");
    }

    // Temporal recall: `since`/`until` accept RFC3339, `YYYY-MM-DD`, or relative
    // durations (`30d`, `3mo`, …) interpreted as "now - duration". `window` is a
    // shorthand preset (`last_week`, `last_month`, `30d`, …). A bad value is a
    // hard parse error — fail-closed so "pre mesec dana" never silently widens.
    let now = chrono::Utc::now();
    let mut t_since = None;
    let mut t_until = None;
    if let Some(w) = args.get("window").and_then(|v| v.as_str()) {
        match altevra_core::time_window::parse_window(w, now) {
            Some(r) => {
                t_since = Some(r.since);
                t_until = Some(r.until);
            }
            None => {
                return McpResponse::error(
                    id,
                    -32602,
                    format!("unknown window '{w}' (try: 24h, 7d, 30d, 3mo, last_week, last_month)"),
                );
            }
        }
    }
    if let Some(s) = args.get("since").and_then(|v| v.as_str()) {
        match altevra_core::time_window::parse_since_until(s, now) {
            Some(t) => t_since = Some(t),
            None => return McpResponse::error(id, -32602, format!("invalid 'since' value '{s}'")),
        }
    }
    if let Some(u) = args.get("until").and_then(|v| v.as_str()) {
        match altevra_core::time_window::parse_since_until(u, now) {
            Some(t) => t_until = Some(t),
            None => return McpResponse::error(id, -32602, format!("invalid 'until' value '{u}'")),
        }
    }

    let result: anyhow::Result<Value> = futures::executor::block_on(async {
        let pool = open_pool(&db_path).await?;
        let repo = altevra_db::SessionsRepository::new(&pool);
        let raw_hits = repo
            .search_turns_with_provenance(query, project, tool, t_since, t_until, limit)
            .await?;
        // R11 #4: gate every hit — never return a turn above the work ceiling or
        // insufficiently redacted, regardless of how well it matched the query.
        let hits: Vec<_> = raw_hits
            .into_iter()
            .filter(|h| turn_exposable(&h.row))
            .collect();
        let now = chrono::Utc::now();
        Ok(serde_json::json!({
            "query": query,
            "count": hits.len(),
            "window": t_since.map(|s| serde_json::json!({
                "since": s,
                "until": t_until,
            })),
            "results": hits.iter().map(|h| {
                // Source-tracing breadcrumb: "claude · altevra · 3w ago" so the
                // caller can show provenance inline without parsing fields.
                let tool_s = h.session_tool.as_deref().unwrap_or("?");
                let proj_s = h.session_project.as_deref().unwrap_or("?");
                let when_h = altevra_core::time_window::humanize_relative(h.row.created_at, now);
                let breadcrumb = format!("{tool_s} · {proj_s} · {when_h}");
                serde_json::json!({
                    "session_id": h.row.session_id,
                    "turn_idx": h.row.turn_idx,
                    "role": h.row.role,
                    "score": h.score,
                    "snippet": h.row.content.chars().take(220).collect::<String>(),
                    "created_at": h.row.created_at,
                    "provenance": {
                        "tool": h.session_tool,
                        "project": h.session_project,
                        "when_human": when_h,
                        "breadcrumb": breadcrumb,
                    },
                })
            }).collect::<Vec<_>>(),
        }))
    });

    match result {
        Ok(v) => McpResponse::ok(id, v),
        Err(e) => McpResponse::error(id, -32603, e.to_string()),
    }
}

/// `recall_window` — recent memory by TIME with NO search query. Lists the most
/// recent recorded turns within a window (newest first), each gated + breadcrumbed.
/// Defaults to `last_week` when no `window`/`since`/`until` is given. Same R11 #4
/// exposure gate as `search_turns` (never returns above-ceiling/unredacted turns).
pub fn handle_recall_window(id: Value, args: &Value) -> McpResponse {
    let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(20);
    let project = args.get("project").and_then(|v| v.as_str());
    let tool = args.get("tool").and_then(|v| v.as_str());
    let db_path = db_path_from_args(args);

    let now = chrono::Utc::now();
    let mut t_since = None;
    let mut t_until = None;
    let mut window_label: Option<String> = None;
    if let Some(w) = args.get("window").and_then(|v| v.as_str()) {
        match altevra_core::time_window::parse_window(w, now) {
            Some(r) => {
                t_since = Some(r.since);
                t_until = Some(r.until);
                window_label = Some(w.to_string());
            }
            None => {
                return McpResponse::error(
                    id,
                    -32602,
                    format!("unknown window '{w}' (try: 24h, 7d, 30d, 3mo, last_week, last_month)"),
                );
            }
        }
    }
    if let Some(s) = args.get("since").and_then(|v| v.as_str()) {
        match altevra_core::time_window::parse_since_until(s, now) {
            Some(t) => t_since = Some(t),
            None => return McpResponse::error(id, -32602, format!("invalid 'since' value '{s}'")),
        }
    }
    if let Some(u) = args.get("until").and_then(|v| v.as_str()) {
        match altevra_core::time_window::parse_since_until(u, now) {
            Some(t) => t_until = Some(t),
            None => return McpResponse::error(id, -32602, format!("invalid 'until' value '{u}'")),
        }
    }
    // No explicit window → default to last_week (the common "what happened lately").
    if t_since.is_none() && t_until.is_none() {
        if let Some(r) = altevra_core::time_window::parse_window("last_week", now) {
            t_since = Some(r.since);
            t_until = Some(r.until);
            window_label = Some("last_week".to_string());
        }
    }

    let result: anyhow::Result<Value> = futures::executor::block_on(async {
        let pool = open_pool(&db_path).await?;
        let repo = altevra_db::SessionsRepository::new(&pool);
        let raw_hits = repo
            .recent_turns_with_provenance(project, tool, t_since, t_until, limit)
            .await?;
        // R11 #4: gate every hit (same as search_turns) — recency never bypasses
        // the exposure ceiling / redaction requirement.
        let hits: Vec<_> = raw_hits
            .into_iter()
            .filter(|h| turn_exposable(&h.row))
            .collect();
        let now = chrono::Utc::now();
        Ok(serde_json::json!({
            "window": window_label,
            "since": t_since,
            "until": t_until,
            "count": hits.len(),
            "results": hits.iter().map(|h| {
                let tool_s = h.session_tool.as_deref().unwrap_or("?");
                let proj_s = h.session_project.as_deref().unwrap_or("?");
                let when_h = altevra_core::time_window::humanize_relative(h.row.created_at, now);
                let breadcrumb = format!("{tool_s} · {proj_s} · {when_h}");
                serde_json::json!({
                    "session_id": h.row.session_id,
                    "turn_idx": h.row.turn_idx,
                    "role": h.row.role,
                    "snippet": h.row.content.chars().take(220).collect::<String>(),
                    "created_at": h.row.created_at,
                    "provenance": {
                        "tool": h.session_tool,
                        "project": h.session_project,
                        "when_human": when_h,
                        "breadcrumb": breadcrumb,
                    },
                })
            }).collect::<Vec<_>>(),
        }))
    });

    match result {
        Ok(v) => McpResponse::ok(id, v),
        Err(e) => McpResponse::error(id, -32603, e.to_string()),
    }
}

/// R11 #4 for durable OBJECTS (the mention-graph targets): a learning carrying a
/// high-water domain (`relationship`/`personal`/`health`/…) is stored Restricted,
/// so the ExposureGate denies it to an MCP/agent caller — a person/health note
/// mentioned by an entity must NOT leak just because it was linked. Same gate the
/// turn reads use, applied with the object's real domain + sensitivity + redaction.
fn object_exposable(
    domain: &str,
    sensitivity: &str,
    redaction_status: &str,
    created_at: chrono::DateTime<chrono::Utc>,
) -> bool {
    use altevra_core::envelope::{Envelope, Provenance, ProvenanceOrigin};
    use altevra_core::safety::{ExposureGate, ExposureRequest};
    use altevra_core::security::Sensitivity;
    use altevra_core::status::RedactionStatus;

    let mut env = Envelope::new(
        "object",
        "learning",
        created_at,
        Provenance::new(ProvenanceOrigin::Imported),
    );
    env.sensitivity = sensitivity.parse::<Sensitivity>().unwrap();
    env.domain = domain.parse::<altevra_core::domain::Domain>().unwrap();
    let redaction = redaction_status
        .parse::<RedactionStatus>()
        .unwrap_or(RedactionStatus::Unscanned);
    ExposureGate::decide(&env, &redaction, &ExposureRequest::default_work()).is_allowed()
}

/// `recall_about { entity, window?, limit?, db_path?, vault? }` — the mention graph
/// over MCP, so Claude Code / Cursor / Codex can ask "what about Đorđe" too (not
/// just the CLI). Resolves the name via the shared dictionary (diacritic/case/
/// inflection-insensitive), returns objects linked through `mentions` edges,
/// recency-sorted with breadcrumbs, EACH passed through the same exposure gate as
/// `search_turns`/`recall_window`. Unknown name → a clean error (no leak).
pub fn handle_recall_about(
    id: Value,
    args: &Value,
    default_vault: &std::path::Path,
) -> McpResponse {
    let entity_name = args.get("entity").and_then(|v| v.as_str()).unwrap_or("");
    if entity_name.trim().is_empty() {
        return McpResponse::error(id, -32602, "entity required");
    }
    let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(20);
    let db_path = db_path_from_args(args);
    let vault: PathBuf = args
        .get("vault")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .unwrap_or_else(|| default_vault.to_path_buf());

    // Temporal window (optional) — same fail-closed parse as the other tools.
    let now = chrono::Utc::now();
    let mut t_since = None;
    let mut t_until = None;
    if let Some(w) = args.get("window").and_then(|v| v.as_str()) {
        match altevra_core::time_window::parse_window(w, now) {
            Some(r) => {
                t_since = Some(r.since);
                t_until = Some(r.until);
            }
            None => {
                return McpResponse::error(
                    id,
                    -32602,
                    format!("unknown window '{w}' (try: 24h, 7d, 30d, 3mo, last_week, last_month)"),
                );
            }
        }
    }

    // Resolve the entity from the shared vault dictionary.
    let dict = altevra_vault::build_dictionary_for_vault(&vault);
    let Some(entity) = altevra_vault::resolve_entity(&dict, entity_name) else {
        // Clean miss — list nothing sensitive, just the not-found fact.
        return McpResponse::ok(
            id,
            serde_json::json!({
                "entity": entity_name,
                "resolved": false,
                "message": format!("no known person/project matching '{entity_name}'"),
                "count": 0,
                "results": [],
            }),
        );
    };
    let entity_id = entity.id.clone();
    let entity_display = entity.name.clone();
    let entity_kind = entity.kind.as_str();

    let result: anyhow::Result<Value> = futures::executor::block_on(async {
        let pool = open_pool(&db_path).await?;
        let mentions = altevra_db::MentionsRepository::new(&pool);
        let learnings = altevra_db::LearningsRepository::new(&pool);
        // Pull a generous candidate set; gate + window-filter, then truncate.
        let sources = mentions
            .objects_mentioning(&entity_id, (limit * 4).max(50))
            .await?;

        let mut results = Vec::new();
        for (otype, oid) in &sources {
            if otype != "learning" {
                continue; // only atomized learning objects carry edges today
            }
            let Some(row) = learnings.get(oid).await? else {
                continue;
            };
            if row.status == "forgotten" {
                continue;
            }
            // Temporal cut on the section's created date (from provenance) if present.
            let when = provenance_created(&row.provenance).unwrap_or(now);
            if let Some(s) = t_since {
                if when < s {
                    continue;
                }
            }
            if let Some(u) = t_until {
                if when >= u {
                    continue;
                }
            }
            // R11 #4: gate the object — a Restricted (high-water) note is withheld.
            if !object_exposable(&row.domain, &row.sensitivity, &row.redaction_status, when) {
                continue;
            }
            let when_h = altevra_core::time_window::humanize_relative(when, now);
            results.push((
                when,
                serde_json::json!({
                    "id": row.id,
                    "title": row.title,
                    "domain": row.domain,
                    "when": when,
                    "when_human": when_h,
                    "snippet": row.body.chars().take(220).collect::<String>(),
                    "breadcrumb": format!("{} · {} · {when_h}", object_kind(&row.tags), row.domain),
                }),
            ));
        }
        results.sort_by_key(|r| std::cmp::Reverse(r.0));
        results.truncate(limit as usize);
        let entries: Vec<Value> = results.into_iter().map(|(_, v)| v).collect();
        Ok(serde_json::json!({
            "entity": entity_name,
            "resolved": true,
            "entity_id": entity_id,
            "entity_name": entity_display,
            "kind": entity_kind,
            "window": t_since.map(|s| serde_json::json!({"since": s, "until": t_until})),
            "count": entries.len(),
            "results": entries,
        }))
    });

    match result {
        Ok(v) => McpResponse::ok(id, v),
        Err(e) => McpResponse::error(id, -32603, e.to_string()),
    }
}

/// `kind:<type>` tag → display kind, else `note`.
fn object_kind(tags_json: &str) -> String {
    if let Ok(tags) = serde_json::from_str::<Vec<String>>(tags_json) {
        for t in tags {
            if let Some(k) = t.strip_prefix("kind:") {
                return k.to_string();
            }
        }
    }
    "note".to_string()
}

/// Pull a `created` YYYY-MM-DD date from a provenance JSON blob (atomize stores
/// the section heading's date there).
fn provenance_created(provenance: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let v: serde_json::Value = serde_json::from_str(provenance).ok()?;
    let s = v.get("created")?.as_str()?;
    let d = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()?;
    Some(d.and_hms_opt(12, 0, 0)?.and_utc())
}

pub fn handle_file_history(id: Value, args: &Value) -> McpResponse {
    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
    let db_path = db_path_from_args(args);

    if path.is_empty() {
        return McpResponse::error(id, -32602, "path required");
    }

    let result: anyhow::Result<Value> = futures::executor::block_on(async {
        let pool = open_pool(&db_path).await?;
        let repo = altevra_db::SessionsRepository::new(&pool);
        let history = repo.file_history(path, limit).await?;
        Ok(serde_json::json!({
            "path": path,
            "count": history.len(),
            "history": history.iter().map(|c| serde_json::json!({
                "id": c.id,
                "session_id": c.session_id,
                "turn_id": c.turn_id,
                "before_hash": c.before_hash,
                "after_hash": c.after_hash,
                "diff_summary": c.diff_summary,
                "actor_id": c.actor_id,
                "created_at": c.created_at,
            })).collect::<Vec<_>>(),
        }))
    });

    match result {
        Ok(v) => McpResponse::ok(id, v),
        Err(e) => McpResponse::error(id, -32603, e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use altevra_db::{FileChangeRow, SessionRow, SessionsRepository, TurnRow};
    use chrono::Utc;
    use tempfile::TempDir;
    use uuid::Uuid;

    async fn seed(db: &std::path::Path) -> Uuid {
        let pool = altevra_db::create_pool(&db.to_string_lossy())
            .await
            .unwrap();
        altevra_db::run_migrations(&pool).await.unwrap();
        let repo = SessionsRepository::new(&pool);
        let s = SessionRow {
            id: Uuid::new_v4(),
            tool: "claude-code".into(),
            project_id: None,
            project_name: Some("altevra".into()),
            started_at: Utc::now(),
            ended_at: None,
            summary: None,
            tokens_in_total: 0,
            tokens_out_total: 0,
            cost_usd_estimate: 0.0,
            turn_count: 0,
            metadata: serde_json::json!({}),
            external_id: None,
            imported_from: None,
            working_dir: None,
        };
        repo.start_session(&s).await.unwrap();
        repo.record_turn(&TurnRow {
            id: Uuid::new_v4(),
            session_id: s.id,
            turn_idx: 0,
            role: "user".into(),
            content: "Talk about GTM strategy and Rust patterns".into(),
            tool_calls: None,
            tool_name: None,
            model: None,
            tokens_in: None,
            tokens_out: None,
            latency_ms: None,
            file_changes: None,
            redacted_count: 0,
            source_tool: Some("claude-code".into()),
            sensitivity: "internal".into(),
            redaction_status: "clean".into(),
            created_at: Utc::now(),
            working_dir: None,
        })
        .await
        .unwrap();
        repo.record_file_change(&FileChangeRow {
            id: Uuid::new_v4(),
            session_id: Some(s.id),
            turn_id: None,
            path: "src/main.rs".into(),
            before_hash: Some("a".into()),
            after_hash: Some("b".into()),
            diff_summary: Some("edit".into()),
            actor_type: "agent".into(),
            actor_id: Some("claude".into()),
            created_at: Utc::now(),
        })
        .await
        .unwrap();
        s.id
    }

    #[tokio::test]
    async fn replay_session_returns_turns() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("a.db");
        let sid = seed(&db).await;
        let args = serde_json::json!({
            "session_id": sid.to_string(),
            "db_path": db.to_string_lossy(),
        });
        let resp = handle_replay_session(serde_json::json!(1), &args);
        assert!(resp.error.is_none(), "{:?}", resp.error);
        let result = resp.result.unwrap();
        assert_eq!(result["session"]["id"], sid.to_string());
        assert!(!result["turns"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn search_turns_returns_match() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("a.db");
        seed(&db).await;
        let args = serde_json::json!({
            "query": "GTM",
            "db_path": db.to_string_lossy(),
        });
        let resp = handle_search_turns(serde_json::json!(1), &args);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert!(result["count"].as_i64().unwrap() >= 1);
    }

    #[tokio::test]
    async fn recall_window_lists_recent_without_query() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("a.db");
        seed(&db).await; // seeds turns at Utc::now() → within last_week
                         // No query, no window → defaults to last_week.
        let args = serde_json::json!({ "db_path": db.to_string_lossy() });
        let resp = handle_recall_window(serde_json::json!(1), &args);
        assert!(resp.error.is_none(), "{:?}", resp.error);
        let result = resp.result.unwrap();
        assert_eq!(result["window"], "last_week", "defaults to last_week");
        assert!(
            result["count"].as_i64().unwrap() >= 1,
            "recent turns listed without a query"
        );
        // every hit carries a provenance breadcrumb.
        let first = &result["results"][0];
        assert!(first["provenance"]["breadcrumb"].as_str().is_some());
    }

    #[tokio::test]
    async fn recall_window_rejects_bad_window() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("a.db");
        seed(&db).await;
        let args = serde_json::json!({ "window": "garbage", "db_path": db.to_string_lossy() });
        let resp = handle_recall_window(serde_json::json!(1), &args);
        // fail-closed: a bad window is an error, never a silently-wide listing.
        assert!(resp.error.is_some());
    }

    #[tokio::test]
    async fn file_history_returns_changes() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("a.db");
        seed(&db).await;
        let args = serde_json::json!({
            "path": "src/main.rs",
            "db_path": db.to_string_lossy(),
        });
        let resp = handle_file_history(serde_json::json!(1), &args);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["count"].as_i64().unwrap(), 1);
    }

    #[test]
    fn turn_exposable_gates_restricted_and_unscanned() {
        let base = |sens: &str, red: &str| TurnRow {
            id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            turn_idx: 0,
            role: "user".into(),
            content: "x".into(),
            tool_calls: None,
            tool_name: None,
            model: None,
            tokens_in: None,
            tokens_out: None,
            latency_ms: None,
            file_changes: None,
            redacted_count: 0,
            source_tool: None,
            sensitivity: sens.into(),
            redaction_status: red.into(),
            created_at: Utc::now(),
            working_dir: None,
        };
        // R11 #4: an MCP caller gets a work ceiling.
        assert!(turn_exposable(&base("internal", "clean")));
        assert!(turn_exposable(&base("internal", "redacted")));
        // restricted (health/personal) content is withheld...
        assert!(!turn_exposable(&base("restricted", "clean")));
        // ...and so is anything not sufficiently redacted (fail-closed).
        assert!(!turn_exposable(&base("internal", "unscanned")));
        // unknown sensitivity ranks max → withheld.
        assert!(!turn_exposable(&base("weird-future-value", "clean")));
    }

    #[test]
    fn object_exposable_gates_high_water() {
        let now = Utc::now();
        // business/internal/clean → exposable.
        assert!(object_exposable("business", "internal", "clean", now));
        // relationship/restricted (a person note) → withheld even if clean.
        assert!(!object_exposable(
            "relationship",
            "restricted",
            "clean",
            now
        ));
        // health/restricted → withheld.
        assert!(!object_exposable("health", "restricted", "clean", now));
        // unscanned → fail-closed.
        assert!(!object_exposable("business", "internal", "unscanned", now));
    }

    /// Seed a learning + a mention edge to `entity_id`. `learning` is
    /// `(id, title, body)`, `class` is `(domain, sensitivity)`. The entity kind is
    /// derived from the `entity_id` prefix (`person:`/`project:`).
    async fn seed_mention(
        db: &std::path::Path,
        learning: (&str, &str, &str),
        class: (&str, &str),
        entity_id: &str,
    ) {
        let (learning_id, title, body) = learning;
        let (domain, sensitivity) = class;
        let entity_kind = entity_id.split(':').next().unwrap_or("person");
        let pool = altevra_db::create_pool(&db.to_string_lossy())
            .await
            .unwrap();
        altevra_db::run_migrations(&pool).await.unwrap();
        let mut row = altevra_db::LearningRow::new(learning_id, title, body);
        row.domain = domain.into();
        row.sensitivity = sensitivity.into();
        row.redaction_status = "clean".into();
        row.categories = "[\"business\"]".into();
        row.tags = "[\"business\",\"kind:decision\"]".into();
        altevra_db::LearningsRepository::new(&pool)
            .insert(&row)
            .await
            .unwrap();
        altevra_db::MentionsRepository::new(&pool)
            .record("learning", learning_id, entity_kind, entity_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn recall_about_returns_linked_objects_and_resolves_diacritic() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("a.db");
        // vault with a People.md so the dictionary resolves; Đorđe comes from the
        // mentor seed (always present).
        let mem = tmp.path().join("Memory");
        std::fs::create_dir_all(&mem).unwrap();
        std::fs::write(mem.join("People.md"), "# People\n\n## Luka — ReVesta\n").unwrap();

        seed_mention(
            &db,
            (
                "capture-decisions-lane-1",
                "Lane split",
                "Đorđe je rekao: prodaja pre build-a.",
            ),
            ("business", "internal"),
            "person:djordje",
        )
        .await;

        // ascii spelling "Djordje" must resolve to the same entity → 1 hit.
        let args = serde_json::json!({
            "entity": "Djordje",
            "db_path": db.to_string_lossy(),
            "vault": tmp.path().to_string_lossy(),
        });
        let resp = handle_recall_about(serde_json::json!(1), &args, tmp.path());
        assert!(resp.error.is_none(), "{:?}", resp.error);
        let r = resp.result.unwrap();
        assert_eq!(r["resolved"], true);
        assert_eq!(r["entity_id"], "person:djordje");
        assert_eq!(r["count"].as_i64().unwrap(), 1);
        assert!(r["results"][0]["breadcrumb"]
            .as_str()
            .unwrap()
            .contains("decision"));
    }

    #[tokio::test]
    async fn recall_about_gates_high_water_object() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("a.db");
        let mem = tmp.path().join("Memory");
        std::fs::create_dir_all(&mem).unwrap();
        std::fs::write(mem.join("People.md"), "# People\n").unwrap();

        // A RESTRICTED relationship note that mentions Đorđe must NOT be returned.
        seed_mention(
            &db,
            (
                "capture-people-secret-1",
                "Private note",
                "sensitive personal context about Đorđe",
            ),
            ("relationship", "restricted"),
            "person:djordje",
        )
        .await;

        let args = serde_json::json!({
            "entity": "Đorđe",
            "db_path": db.to_string_lossy(),
            "vault": tmp.path().to_string_lossy(),
        });
        let resp = handle_recall_about(serde_json::json!(1), &args, tmp.path());
        let r = resp.result.unwrap();
        assert_eq!(r["resolved"], true);
        assert_eq!(
            r["count"].as_i64().unwrap(),
            0,
            "high-water object must be exposure-gated, never leaked via mentions"
        );
    }

    #[tokio::test]
    async fn recall_about_unknown_entity_is_clean_miss() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("a.db");
        let mem = tmp.path().join("Memory");
        std::fs::create_dir_all(&mem).unwrap();
        std::fs::write(mem.join("People.md"), "# People\n").unwrap();
        let args = serde_json::json!({
            "entity": "Nobody McNotreal",
            "db_path": db.to_string_lossy(),
            "vault": tmp.path().to_string_lossy(),
        });
        let resp = handle_recall_about(serde_json::json!(1), &args, tmp.path());
        assert!(resp.error.is_none());
        let r = resp.result.unwrap();
        assert_eq!(r["resolved"], false);
        assert_eq!(r["count"].as_i64().unwrap(), 0);
        assert!(r["message"].as_str().unwrap().contains("no known"));
    }

    #[test]
    fn recall_about_requires_entity() {
        let resp = handle_recall_about(
            serde_json::json!(1),
            &serde_json::json!({}),
            std::path::Path::new("."),
        );
        assert!(resp.error.is_some());
    }
}

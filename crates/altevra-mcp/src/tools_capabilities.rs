use serde_json::Value;
use std::path::PathBuf;

use crate::server::McpResponse;

const CAP_PATH: &str = ".altevra/state/capabilities.json";
const KGAP_PATH: &str = ".altevra/state/knowledge_gaps.jsonl";
const CGAP_PATH: &str = ".altevra/state/capability_gaps.jsonl";
const REVIEW_PATH: &str = ".altevra/state/review_items.jsonl";

/// Resolve the SQLite path for DB-backed handlers: explicit `db_path` arg wins,
/// otherwise fall back to the core default. Mirrors `tools_sessions`.
fn db_path_from_args(args: &Value) -> PathBuf {
    args.get("db_path")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .unwrap_or_else(altevra_core::default_db_path)
}

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

/// HP-1 (write-proposal-only): an MCP/agent caller asks core to consider
/// forgetting `object_id`. This NEVER executes a forget — it ONLY drops a
/// `review_items` row (`kind="forget_request"`, `status="proposed"`) that a
/// presence-checked human path later acts on (or doesn't). The object is not
/// touched, tombstoned, or marked. The tool name deliberately uses `request`
/// (not `execute` / `apply` / `approve` / `grant`) so the HP-1 lock regression
/// test in `server.rs` keeps holding.
pub fn handle_request_forget(id: Value, args: &Value) -> McpResponse {
    let object_type = match args["object_type"].as_str() {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => return McpResponse::error(id, -32602, "missing 'object_type'"),
    };
    let object_id = match args["object_id"].as_str() {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => return McpResponse::error(id, -32602, "missing 'object_id'"),
    };
    let reason = args["reason"].as_str().unwrap_or("").to_string();
    let db_path = db_path_from_args(args);

    // body carries the structured request so a reviewer / curator can resolve
    // {object_type, object_id} later without parsing free text.
    let body = serde_json::json!({
        "object_type": &object_type,
        "object_id": &object_id,
        "reason": &reason,
    })
    .to_string();
    let title = format!("forget_request: {object_type}/{object_id}");

    let result: anyhow::Result<Value> = std::thread::spawn(move || -> anyhow::Result<Value> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        rt.block_on(async move {
            let pool = altevra_db::create_pool(&db_path.to_string_lossy()).await?;
            altevra_db::run_migrations(&pool).await?;
            let new_id = uuid::Uuid::new_v4();
            let now = chrono::Utc::now();
            let row = altevra_db::ReviewItemRow {
                id: new_id,
                project_id: None,
                kind: "forget_request".to_string(),
                title: title.clone(),
                body: Some(body.clone()),
                // HP-1: status is `proposed` — never `approved`/`applied`. A
                // presence-gated human path is the only thing that may move it.
                status: "proposed".to_string(),
                created_at: now,
                metadata: serde_json::json!({
                    "object_type": object_type,
                    "object_id": object_id,
                    "reason": reason,
                    "source": "mcp:request_forget",
                }),
            };
            altevra_db::TasksRepository::new(&pool)
                .create_review_item(&row)
                .await?;
            Ok(serde_json::json!({
                "review_item_id": new_id.to_string(),
                "status": "proposed",
                "kind": "forget_request",
            }))
        })
    })
    .join()
    .unwrap_or_else(|_| Err(anyhow::anyhow!("db worker thread panicked")));

    match result {
        Ok(v) => McpResponse::ok(id, v),
        Err(e) => McpResponse::error(id, -32000, format!("request_forget failed: {e}")),
    }
}

/// HP-1 (write-proposal-only): MCP entrypoint for an AI tool (Claude / Cursor
/// / Codex) to surface a learning to Altevra. Writes a `proposals` row via
/// `ProposalsRepository::insert` — SI-9 (tier re-derive from `kind`) and SI-13
/// (dedup_hash collision merges, never a 2nd row) fire automatically. Status
/// is always `proposed`; this tool NEVER applies, approves, or grants.
pub fn handle_propose_improvement(id: Value, args: &Value) -> McpResponse {
    let kind = match args["kind"].as_str() {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => return McpResponse::error(id, -32602, "missing 'kind'"),
    };
    let title = match args["title"].as_str() {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => return McpResponse::error(id, -32602, "missing 'title'"),
    };
    let body = args["body"].as_str().unwrap_or("").to_string();
    let evidence_refs: Vec<String> = args["evidence_refs"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let db_path = db_path_from_args(args);

    // Deterministic dedup key: same (kind, title) → same hash → merge (SI-13).
    let dedup_hash = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        "mcp:propose_improvement".hash(&mut h);
        0u8.hash(&mut h);
        kind.hash(&mut h);
        0u8.hash(&mut h);
        title.hash(&mut h);
        format!("mcp:{:016x}", h.finish())
    };

    let np = altevra_db::NewProposal {
        kind: kind.clone(),
        title: title.clone(),
        body,
        source_mode: Some("mcp:propose_improvement".to_string()),
        dedup_hash,
        evidence_refs,
        // Agent-asserted sensitivity is rejected; SI-9 derives tier from `kind`
        // (+ these flags which the MCP surface keeps `false` — Altevra core is
        // the only place that may upgrade tier based on richer context).
        touches_sensitive: false,
        touches_constitutional: false,
    };

    let result: anyhow::Result<Value> = std::thread::spawn(move || -> anyhow::Result<Value> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        rt.block_on(async move {
            let pool = altevra_db::create_pool(&db_path.to_string_lossy()).await?;
            altevra_db::run_migrations(&pool).await?;
            let repo = altevra_db::ProposalsRepository::new(&pool);
            let (pid, is_new) = repo.insert(&np).await?;
            // Read back the row so the caller sees the SI-9-derived tier.
            let row = repo.get(&pid).await?;
            let risk_tier = row.as_ref().map(|r| r.risk_tier.clone()).unwrap_or_default();
            let evidence_count = row.as_ref().map(|r| r.evidence_count).unwrap_or(1);
            Ok(serde_json::json!({
                "proposal_id": pid,
                "status": "proposed",
                "risk_tier": risk_tier,
                "is_new": is_new,
                "evidence_count": evidence_count,
            }))
        })
    })
    .join()
    .unwrap_or_else(|_| Err(anyhow::anyhow!("db worker thread panicked")));

    match result {
        Ok(v) => McpResponse::ok(id, v),
        Err(e) => McpResponse::error(id, -32000, format!("propose_improvement failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

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

    /// HP-1: `request_forget` only writes a `review_items` row with
    /// `kind=forget_request`, `status=proposed`. It NEVER touches the object
    /// it names. Verified by: (a) the row lands with the right shape, (b) no
    /// matching object row gets a tombstone / status change (we just check the
    /// existing `proposals` table is untouched — there are zero apply-side
    /// effects here).
    #[test]
    fn request_forget_writes_review_item_only() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("altevra.db");
        let args = serde_json::json!({
            "object_type": "decision",
            "object_id": "11111111-1111-1111-1111-111111111111",
            "reason": "outdated, no longer applies",
            "db_path": db.to_string_lossy(),
        });
        let resp = handle_request_forget(Value::from(1), &args);
        assert!(resp.error.is_none(), "handler errored: {:?}", resp.error);
        let result = resp.result.unwrap();
        assert_eq!(result["status"], "proposed");
        assert_eq!(result["kind"], "forget_request");
        let rid = result["review_item_id"].as_str().unwrap().to_string();

        // Verify the row landed in `review_items` and nothing else fired.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let pool = altevra_db::create_pool(&db.to_string_lossy()).await.unwrap();
            altevra_db::run_migrations(&pool).await.unwrap();
            let repo = altevra_db::TasksRepository::new(&pool);
            let parsed = uuid::Uuid::parse_str(&rid).unwrap();
            let row = repo.get_review_item(parsed).await.unwrap().unwrap();
            assert_eq!(row.kind, "forget_request");
            assert_eq!(row.status, "proposed");
            assert!(row.title.contains("decision"));
            assert!(row.title.contains("11111111-1111-1111-1111-111111111111"));
            // No proposals were written.
            let proposals = altevra_db::ProposalsRepository::new(&pool)
                .list(None, None)
                .await
                .unwrap();
            assert_eq!(proposals.len(), 0, "request_forget MUST NOT write a proposal");
        });
    }

    /// `propose_improvement` writes a row via `ProposalsRepository::insert`, so
    /// SI-9 (tier re-derive from `kind`) and SI-13 (dedup_hash merge) fire.
    /// Calling it twice with the same `(kind, title)` MUST merge, not create a
    /// 2nd row.
    #[test]
    fn propose_improvement_writes_proposal_row() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("altevra.db");

        // First call: "skill" kind → SI-9 derives tier1.
        let args = serde_json::json!({
            "kind": "skill",
            "title": "split long prompt into stages",
            "body": "Observed prompt > 4k tokens; staged version improved latency.",
            "evidence_refs": ["turn:abc", "session:xyz"],
            "db_path": db.to_string_lossy(),
        });
        let resp = handle_propose_improvement(Value::from(1), &args);
        assert!(resp.error.is_none(), "handler errored: {:?}", resp.error);
        let result = resp.result.unwrap();
        assert_eq!(result["status"], "proposed");
        assert_eq!(
            result["risk_tier"], "tier1",
            "SI-9: 'skill' must derive to tier1 regardless of caller"
        );
        assert_eq!(result["is_new"], true);
        assert_eq!(result["evidence_count"], 1);
        let first_id = result["proposal_id"].as_str().unwrap().to_string();

        // Second call: same (kind, title) → dedup_hash collision → merge.
        let resp2 = handle_propose_improvement(Value::from(2), &args);
        let result2 = resp2.result.unwrap();
        assert_eq!(result2["is_new"], false, "SI-13: collision must merge");
        assert_eq!(
            result2["proposal_id"].as_str().unwrap(),
            first_id,
            "SI-13: collision returns existing id"
        );
        assert_eq!(result2["evidence_count"], 2, "evidence_count increments");

        // Verify exactly 1 proposal row exists.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let pool = altevra_db::create_pool(&db.to_string_lossy()).await.unwrap();
            altevra_db::run_migrations(&pool).await.unwrap();
            let repo = altevra_db::ProposalsRepository::new(&pool);
            let rows = repo.list(None, None).await.unwrap();
            assert_eq!(rows.len(), 1, "SI-13: dedup collision must NOT create a 2nd row");
            assert_eq!(rows[0].status, "proposed");
            assert_eq!(rows[0].risk_tier, "tier1");
            assert_eq!(rows[0].source_mode.as_deref(), Some("mcp:propose_improvement"));
        });
    }

    #[test]
    fn request_forget_missing_object_type_errors() {
        let resp = handle_request_forget(
            Value::from(1),
            &serde_json::json!({"object_id": "x", "reason": "y"}),
        );
        assert!(resp.error.is_some());
    }

    #[test]
    fn propose_improvement_missing_kind_errors() {
        let resp =
            handle_propose_improvement(Value::from(1), &serde_json::json!({"title": "t"}));
        assert!(resp.error.is_some());
    }

    /// HP-1 lock co-located with the new tool surface: neither `request_forget`
    /// nor `propose_improvement` (and the existing `create_review_item`) name
    /// matches the forbidden-verb set the server-level regression test scans
    /// for (approve|apply|grant|forget_execute|revoke|set_policy|legal_hold|
    /// execute). `request` and `propose` are explicitly NOT in that set —
    /// they're the write-proposal-only seam HP-1 contemplates.
    #[test]
    fn hp1_lock_still_holds_for_new_tools() {
        let forbidden = [
            "approve",
            "apply",
            "grant",
            "forget_execute",
            "forget-execute",
            "set_policy",
            "set-policy",
            "revoke",
            "legal_hold",
            "execute",
        ];
        for name in ["request_forget", "propose_improvement", "create_review_item"] {
            for f in forbidden {
                assert!(
                    !name.contains(f),
                    "HP-1 violation: tool '{name}' contains forbidden verb '{f}'"
                );
            }
        }
    }
}

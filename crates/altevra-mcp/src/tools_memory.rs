use altevra_memory::{ingest_file, SearchIndex};
use altevra_vault::scan_vault;
use serde_json::Value;

use crate::server::McpResponse;

pub fn handle_search_memory(id: Value, args: &Value) -> McpResponse {
    let query = match args["query"].as_str() {
        Some(q) => q,
        None => return McpResponse::error(id, -32602, "missing 'query'"),
    };
    let vault = args["vault"].as_str().unwrap_or(".");
    let limit = args["limit"].as_u64().unwrap_or(5) as usize;

    let files = match scan_vault(std::path::Path::new(vault)) {
        Ok(f) => f,
        Err(e) => return McpResponse::error(id, -32000, format!("vault scan failed: {e}")),
    };
    let mut index = SearchIndex::new();
    for f in &files {
        if let Ok(doc) = ingest_file(&f.path, 2000) {
            index.add_document(doc);
        }
    }
    let hits = index.search(query, limit);
    let payload = serde_json::json!({
        "query": query,
        "total_chunks": index.len(),
        "hits": hits.iter().map(|h| serde_json::json!({
            "chunk_id": h.chunk_id,
            "source": h.source_path,
            "heading_path": h.heading_path,
            "score": h.score,
            "snippet": h.snippet,
        })).collect::<Vec<_>>(),
    });
    McpResponse::ok(id, payload)
}

pub fn handle_get_project_context(id: Value, args: &Value) -> McpResponse {
    let vault = args["vault"].as_str().unwrap_or(".");
    let project = args["project"].as_str();

    let files = match scan_vault(std::path::Path::new(vault)) {
        Ok(f) => f,
        Err(e) => return McpResponse::error(id, -32000, format!("vault scan failed: {e}")),
    };
    let relevant: Vec<_> = files
        .into_iter()
        .filter(|f| {
            project
                .map(|p| f.path.to_string_lossy().contains(p))
                .unwrap_or(true)
        })
        .collect();

    McpResponse::ok(
        id,
        serde_json::json!({
            "project": project,
            "vault_root": vault,
            "file_count": relevant.len(),
            "files": relevant.iter().take(50).map(|f| serde_json::json!({
                "path": f.path,
                "section": f.section,
                "size_bytes": f.size_bytes,
            })).collect::<Vec<_>>(),
        }),
    )
}

pub fn handle_get_context_packet(id: Value, args: &Value) -> McpResponse {
    let vault = args["vault"].as_str().unwrap_or(".");
    let agent = args["agent"].as_str().unwrap_or("default");

    let files = scan_vault(std::path::Path::new(vault)).unwrap_or_default();
    let sections: std::collections::BTreeSet<_> =
        files.iter().filter_map(|f| f.section.clone()).collect();

    // T-INV14: a REAL gated packet over object_index via PacketCompiler +
    // ExposureGate (R12 tag/FTS/graph, NO vectors). An MCP/agent caller gets a
    // work ceiling, so restricted (health/personal) objects are excluded.
    // Fault-tolerant: any error → empty packet, never fails the MCP response, so
    // the existing vault-stats fields are always returned (no regression).
    let db_path = args["db_path"]
        .as_str()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(altevra_core::default_db_path);
    let query_terms: Vec<String> = args["query"]
        .as_str()
        .map(|q| q.split_whitespace().map(String::from).collect())
        .unwrap_or_default();

    McpResponse::ok(
        id,
        serde_json::json!({
            "agent": agent,
            "vault_root": vault,
            "file_count": files.len(),
            "sections": sections,
            "packet": gated_packet(&db_path, &query_terms),
        }),
    )
}

fn empty_packet() -> Value {
    serde_json::json!({"items": [], "excluded": 0, "tokens_used": 0, "truncated": false})
}

/// Compile a gated context packet from `object_index` candidates (T-INV14). The
/// ExposureGate filters by ceiling/scope/redaction before ranking; restricted and
/// unscanned objects never appear. Any error yields an empty packet.
///
/// Runs on a dedicated thread with its own current-thread Tokio runtime so it is
/// safe to call from any context (a parent runtime, a sync handler, or a test) —
/// `block_on` on a borrowed worker thread would deadlock sqlx's background tasks.
fn gated_packet(db_path: &std::path::Path, query_terms: &[String]) -> Value {
    let db = db_path.to_path_buf();
    let terms = query_terms.to_vec();
    // Runs on a dedicated thread with its own current-thread runtime (see the
    // doc comment above) and delegates to the SINGLE shared builder so CLI + MCP
    // cannot drift (INV-14 parity).
    let joined = std::thread::spawn(move || -> anyhow::Result<Value> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        rt.block_on(async move {
            let pool = altevra_db::create_pool(&db.to_string_lossy()).await?;
            altevra_db::run_migrations(&pool).await?;
            let pkt = crate::packet_build::compile_gated_packet(&pool, &terms, 8000).await?;
            if pkt.items.is_empty() && pkt.excluded.is_empty() {
                return Ok(empty_packet());
            }
            Ok(serde_json::json!({
                "items": pkt.items.iter().map(|i| serde_json::json!({
                    "type": i.object_type,
                    "id": i.object_id,
                    "title": i.title,
                    "rank": i.rank,
                    "sensitivity": i.sensitivity.to_string(),
                    "why": i.why.rule,
                })).collect::<Vec<_>>(),
                "excluded": pkt.excluded.len(),
                "tokens_used": pkt.tokens_used,
                "truncated": pkt.truncated,
            }))
        })
    })
    .join();
    joined
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or_else(empty_packet)
}

pub fn handle_get_source_of_truth(id: Value, args: &Value) -> McpResponse {
    let query = args["query"].as_str().unwrap_or("");
    let vault = args["vault"].as_str().unwrap_or(".");

    // Source-of-truth lookup: prefer 08-decisions/ then 06-skills/ then 09-changes/.
    let priority_sections = ["08-decisions", "06-skills", "09-changes"];
    let files = scan_vault(std::path::Path::new(vault)).unwrap_or_default();
    let mut sot: Vec<_> = files
        .into_iter()
        .filter(|f| {
            f.section
                .as_deref()
                .map(|s| priority_sections.contains(&s))
                .unwrap_or(false)
        })
        .collect();

    sot.sort_by_key(|f| {
        f.section
            .as_ref()
            .and_then(|s| priority_sections.iter().position(|p| p == s))
            .unwrap_or(usize::MAX)
    });

    McpResponse::ok(
        id,
        serde_json::json!({
            "query": query,
            "sources": sot.iter().take(20).map(|f| serde_json::json!({
                "path": f.path,
                "section": f.section,
            })).collect::<Vec<_>>(),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_missing_query_errors() {
        let resp = handle_search_memory(Value::from(1), &serde_json::json!({}));
        assert!(resp.error.is_some());
    }

    #[test]
    fn context_packet_returns_structure() {
        let tmp = tempfile::TempDir::new().unwrap();
        let resp = handle_get_context_packet(
            Value::from(1),
            &serde_json::json!({"vault": tmp.path().to_string_lossy()}),
        );
        assert!(resp.error.is_none());
    }
}

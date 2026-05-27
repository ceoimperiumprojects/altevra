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

    McpResponse::ok(
        id,
        serde_json::json!({
            "agent": agent,
            "vault_root": vault,
            "file_count": files.len(),
            "sections": sections,
        }),
    )
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

use altevra_core::envelope::{Envelope, Provenance, ProvenanceOrigin};
use altevra_core::packet::{PacketCandidate, PacketCompiler, PacketRequest};
use altevra_core::safety::ExposureRequest;
use altevra_core::security::Sensitivity;
use altevra_core::status::{ObjectStatus, RedactionStatus};
use altevra_core::Domain;
use altevra_db::{ExposureAudit, ExposureDecisionsRepository, ObjectIndexRepository};
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
    let joined = std::thread::spawn(move || -> anyhow::Result<Value> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        rt.block_on(async move {
            let pool = altevra_db::create_pool(&db.to_string_lossy()).await?;
            altevra_db::run_migrations(&pool).await?;
            let rows = ObjectIndexRepository::new(&pool).candidates(None).await?;
            if rows.is_empty() {
                return Ok(empty_packet());
            }
            let now = rows.iter().map(|r| r.updated_at).max().unwrap();
            let candidates: Vec<PacketCandidate> = rows.iter().map(row_to_candidate).collect();
            let req = PacketRequest {
                intent: "context".into(),
                project: None,
                query_terms: terms,
                exposure: ExposureRequest::default_work(),
                token_budget: 8000,
            };
            let pkt = PacketCompiler::compile(&candidates, &req, now);

            // R5 audit: every packet compile emits ONE content-free aggregate row
            // to exposure_decisions (append-only, never auto-purged). The compiler
            // stays pure (no db dep); the handler does the write here from the
            // already-compiled packet + request. Content-free by construction —
            // counts + ceiling + why-excluded aggregate, NEVER object ids/titles
            // of denied items (§2.13 no existence leak).
            let mut by_reason: std::collections::BTreeMap<String, usize> =
                std::collections::BTreeMap::new();
            for ex in &pkt.excluded {
                *by_reason.entry(ex.reason.clone()).or_insert(0) += 1;
            }
            let mut redaction_counts: std::collections::BTreeMap<String, usize> =
                std::collections::BTreeMap::new();
            // The packet items are the admitted candidates; record their redaction
            // mix by re-reading the candidate verdicts (no object id is stored).
            for item in &pkt.items {
                if let Some(c) = candidates.iter().find(|c| c.envelope.id == item.object_id) {
                    *redaction_counts
                        .entry(c.redaction_status.to_string())
                        .or_insert(0) += 1;
                }
            }
            let audit = ExposureAudit {
                packet_id: None,
                sensitivity_ceiling: req.exposure.sensitivity_ceiling.to_string(),
                domain_scope: req
                    .exposure
                    .domain_scope
                    .iter()
                    .map(|d| d.to_string())
                    .collect(),
                included_count: pkt.items.len(),
                excluded_count: pkt.excluded.len(),
                excluded_by_reason: by_reason.into_iter().collect(),
                redaction_counts: redaction_counts.into_iter().collect(),
                truncated: pkt.truncated,
            };
            // Fault-tolerant: an audit-write failure must not break the response,
            // but it is the natural, always-attempted side effect of a compile.
            let _ = ExposureDecisionsRepository::new(&pool).insert(&audit).await;

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

fn row_to_candidate(r: &altevra_db::ObjectIndexRow) -> PacketCandidate {
    let mut e = Envelope::new(
        &r.id,
        &r.object_type,
        r.updated_at,
        Provenance::new(ProvenanceOrigin::Imported),
    );
    e.domain = r.domain.parse::<Domain>().unwrap();
    e.sensitivity = r.sensitivity.parse::<Sensitivity>().unwrap();
    e.status = r.status.parse::<ObjectStatus>().unwrap();
    let categories: Vec<String> = serde_json::from_str(&r.categories).unwrap_or_default();
    PacketCandidate::new(
        e,
        r.title.clone().unwrap_or_default(),
        categories,
        r.redaction_status
            .parse::<RedactionStatus>()
            .unwrap_or(RedactionStatus::Unscanned),
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

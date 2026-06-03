//! Shared gated-packet builder (INV-14 parity).
//!
//! The ONE place that turns `object_index` + `object_fts` + the `relations`
//! graph into a gated [`ContextPacket`]. Both the MCP `get_context_packet`
//! handler and the CLI `altevra context` command call this so they CANNOT drift:
//! same candidates, same bm25/graph enrichment, same `ExposureRequest`, same
//! compile, same R5 audit write.
//!
//! Layering (R12, vector-free):
//!   1. `ObjectIndexRepository::candidates` → the candidate pool.
//!   2. bm25 — `FtsRepository::search` scores the query against the FTS substrate;
//!      we attach the raw bm25 to the matching candidates. PRECOMPUTED here so the
//!      compiler stays db-free (it only rank-normalizes + fuses).
//!   3. graph — the FTS top-hits are the query "anchors"; a candidate's graph
//!      signal is the count of anchor objects it shares a mentioned entity with
//!      (`MentionsRepository::entities_for` intersection). Also precomputed here.
//!   4. `PacketCompiler::compile` fuses + gates (ExposureGate strictly first).
//!   5. R5 audit: one content-free aggregate row per compile (append-only).
//!
//! Determinism: candidate order, bm25 ordering, and the id tie-break inside the
//! compiler make the resulting packet byte-equal across runs.

use std::collections::{BTreeMap, BTreeSet};

use altevra_core::envelope::{Envelope, Provenance, ProvenanceOrigin};
use altevra_core::packet::{ContextPacket, PacketCandidate, PacketCompiler, PacketRequest};
use altevra_core::safety::ExposureRequest;
use altevra_core::security::Sensitivity;
use altevra_core::status::{ObjectStatus, RedactionStatus};
use altevra_core::Domain;
use altevra_db::{
    ExposureAudit, ExposureDecisionsRepository, FtsRepository, MentionsRepository,
    ObjectIndexRepository, ObjectIndexRow,
};
use sqlx::SqlitePool;

/// How many FTS hits define the query's bm25 ranking + the graph anchor set.
const FTS_HORIZON: i64 = 50;

/// Build the gated context packet over an existing pool (caller owns migrations).
///
/// `query_terms` are plain terms (the FTS layer ANDs them). Returns the compiled
/// packet AND attempts the R5 audit write (fault-tolerant: an audit-write failure
/// never propagates — the packet is still returned). The compiler is pure; all
/// bm25/graph computation happens here in the db layer.
pub async fn compile_gated_packet(
    pool: &SqlitePool,
    query_terms: &[String],
    token_budget: usize,
) -> anyhow::Result<ContextPacket> {
    let rows = ObjectIndexRepository::new(pool).candidates(None).await?;
    if rows.is_empty() {
        return Ok(ContextPacket {
            items: Vec::new(),
            excluded: Vec::new(),
            tokens_used: 0,
            truncated: false,
        });
    }
    // Deterministic `now`: the newest candidate timestamp (no wall-clock in the
    // compile path → byte-equal packets), matching the MCP handler's prior choice.
    let now = rows.iter().map(|r| r.updated_at).max().unwrap();

    // ---- bm25: raw SQLite score per candidate id (lower = better; None = miss) ----
    let query = query_terms.join(" ");
    let fts = FtsRepository::new(pool);
    let hits = if query.trim().is_empty() {
        Vec::new()
    } else {
        fts.search(&query, FTS_HORIZON).await.unwrap_or_default()
    };
    let bm25_by_id: BTreeMap<String, f64> =
        hits.iter().map(|h| (h.object_id.clone(), h.score)).collect();

    // ---- graph: anchors = the FTS top-hit objects; a candidate's signal is the
    // count of anchors it shares a mentioned entity with (relations 'mentions'
    // edges). Precomputed once: anchor entity-sets, then per-candidate overlap. ----
    let mentions = MentionsRepository::new(pool);
    let mut anchor_entity_sets: Vec<(String, BTreeSet<String>)> = Vec::new();
    for h in &hits {
        let ents = mentions
            .entities_for(&h.object_type, &h.object_id)
            .await
            .unwrap_or_default();
        if !ents.is_empty() {
            anchor_entity_sets.push((h.object_id.clone(), ents.into_iter().collect()));
        }
    }

    // ---- assemble candidates with the precomputed signals ----
    let mut candidates: Vec<PacketCandidate> = Vec::with_capacity(rows.len());
    for r in &rows {
        let mut c = row_to_candidate(r);
        if let Some(score) = bm25_by_id.get(&r.id) {
            c = c.with_bm25(*score);
        }
        // graph signal: number of DISTINCT anchor objects (other than itself) this
        // candidate shares at least one mentioned entity with.
        if !anchor_entity_sets.is_empty() {
            let my_ents: BTreeSet<String> = mentions
                .entities_for(&r.object_type, &r.id)
                .await
                .unwrap_or_default()
                .into_iter()
                .collect();
            if !my_ents.is_empty() {
                let shared = anchor_entity_sets
                    .iter()
                    .filter(|(anchor_id, _)| anchor_id != &r.id)
                    .filter(|(_, ents)| ents.intersection(&my_ents).next().is_some())
                    .count();
                if shared > 0 {
                    c = c.with_graph_signal(shared as f64);
                }
            }
        }
        candidates.push(c);
    }

    let req = PacketRequest {
        intent: "context".into(),
        project: None,
        query_terms: query_terms.to_vec(),
        exposure: ExposureRequest::default_work(),
        token_budget,
    };
    let pkt = PacketCompiler::compile(&candidates, &req, now);

    // ---- R5 audit: ONE content-free aggregate row per compile (append-only,
    // never auto-purged). Content-free by construction — counts + ceiling + the
    // why-excluded aggregate + the admitted redaction mix, NEVER an object id or
    // title of any denied candidate (§2.13 no existence leak). ----
    write_exposure_audit(pool, &pkt, &req, &candidates).await;

    Ok(pkt)
}

/// Map a denormalized index row to a gate-relevant candidate (no signals yet).
fn row_to_candidate(r: &ObjectIndexRow) -> PacketCandidate {
    let mut e = Envelope::new(
        &r.id,
        &r.object_type,
        r.updated_at,
        Provenance::new(ProvenanceOrigin::Imported),
    );
    e.domain = r.domain.parse::<Domain>().unwrap_or(Domain::Business);
    e.sensitivity = r
        .sensitivity
        .parse::<Sensitivity>()
        .unwrap_or(Sensitivity::Internal);
    e.status = r.status.parse::<ObjectStatus>().unwrap_or(ObjectStatus::Active);
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

/// Write the content-free R5 aggregate for a compiled packet. Fault-tolerant.
async fn write_exposure_audit(
    pool: &SqlitePool,
    pkt: &ContextPacket,
    req: &PacketRequest,
    candidates: &[PacketCandidate],
) {
    let mut by_reason: BTreeMap<String, usize> = BTreeMap::new();
    for ex in &pkt.excluded {
        *by_reason.entry(ex.reason.clone()).or_insert(0) += 1;
    }
    let mut redaction_counts: BTreeMap<String, usize> = BTreeMap::new();
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
    let _ = ExposureDecisionsRepository::new(pool).insert(&audit).await;
}

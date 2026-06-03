//! Context Packet Compiler (working draft §3, RECONCILIATION R12).
//!
//! Turns broad capture into precise, source-backed, sensitivity-safe context.
//! Two strictly separated layers (the core anti-RAG-soup principle):
//!
//!  - **Layer A — gates** (hard, boolean): the [`ExposureGate`] decides
//!    inclusion by scope/sensitivity/lifecycle/redaction. A gate failure is
//!    NEVER overridden by relevance.
//!  - **Layer B — relevance** (soft): ranks SURVIVORS only, by the R12 fusion
//!    `f(bm25, tag_match, graph, recency)` (NO vectors — deterministic, no model).
//!    bm25 and the graph weight are PRECOMPUTED by the caller (db lives outside
//!    this crate) and carried on the candidate; the compiler only *consumes* and
//!    fuses them. bm25 is rank-normalized among survivors so the score is stable
//!    across SQLite versions (only the bm25 ordering matters, not its magnitude).
//!
//! Every item carries a resolvable `object_ref`; every inclusion AND exclusion
//! is explained. Deterministic given the same candidates + request.

use crate::envelope::Envelope;
use crate::safety::{DenyReason, ExposureGate, ExposureRequest};
use crate::security::Sensitivity;
use crate::status::RedactionStatus;
use chrono::{DateTime, Utc};

/// A candidate object for packet compilation (from `object_index`).
#[derive(Debug, Clone)]
pub struct PacketCandidate {
    pub envelope: Envelope,
    pub title: String,
    /// Governed categories + tags used for the tag-match relevance signal.
    pub categories: Vec<String>,
    /// The candidate's redaction verdict — the gate fails closed on anything
    /// other than clean/redacted (R11 #8: was hard-coded `None` → fail-open).
    pub redaction_status: RedactionStatus,
    /// Raw SQLite bm25 score for this candidate against the query, PRECOMPUTED by
    /// the caller from `FtsRepository::search` (lower/more-negative = better match;
    /// `None` = the candidate was not an FTS hit). The compiler keeps db-free: it
    /// only rank-normalizes this among survivors (magnitude is version-unstable,
    /// ordering is not). `None` contributes a zero bm25 component.
    pub bm25: Option<f64>,
    /// Graph signal: PRECOMPUTED count/weight of `relations` edges from this
    /// candidate to the query-anchor object(s) (e.g. shared mentioned entities),
    /// supplied by the caller. The compiler only consumes it; `0.0` = no edge.
    pub graph_signal: f64,
}

impl PacketCandidate {
    /// Build a candidate from the gate-relevant fields, with no retrieval signals
    /// (`bm25 = None`, `graph_signal = 0.0`). Callers layer signals on via
    /// [`PacketCandidate::with_bm25`] / [`PacketCandidate::with_graph_signal`].
    pub fn new(
        envelope: Envelope,
        title: impl Into<String>,
        categories: Vec<String>,
        redaction_status: RedactionStatus,
    ) -> Self {
        Self {
            envelope,
            title: title.into(),
            categories,
            redaction_status,
            bm25: None,
            graph_signal: 0.0,
        }
    }

    /// Attach the precomputed raw bm25 score (builder-style).
    pub fn with_bm25(mut self, bm25: f64) -> Self {
        self.bm25 = Some(bm25);
        self
    }

    /// Attach the precomputed graph signal (builder-style).
    pub fn with_graph_signal(mut self, graph_signal: f64) -> Self {
        self.graph_signal = graph_signal;
        self
    }
}

/// The retrieval request (lightweight; full §3.4 RetrievalRequest is a superset).
#[derive(Debug, Clone)]
pub struct PacketRequest {
    pub intent: String,
    pub project: Option<String>,
    /// Free-text/tag query terms used for the tag-match signal.
    pub query_terms: Vec<String>,
    pub exposure: ExposureRequest,
    pub token_budget: usize,
}

/// Why an item made it into the packet.
#[derive(Debug, Clone, PartialEq)]
pub struct WhyIncluded {
    pub rule: String, // "bm25" | "tag_match" | "graph" | "recency"
    pub fused_score: f64,
}

/// A single included item, with its source ref + explanation.
#[derive(Debug, Clone)]
pub struct ContextPacketItem {
    pub object_type: String,
    pub object_id: String,
    pub title: String,
    pub rank: usize,
    pub sensitivity: Sensitivity,
    pub fused_score: f64,
    pub why: WhyIncluded,
    /// Approx token cost (chars/4), for budgeting.
    pub token_count: usize,
}

/// A candidate that did NOT make it — typed, non-leaking reason.
///
/// For denials that would reveal the existence of a higher-classified item
/// (over-ceiling / out-of-scope), `object_type`/`object_id` are `None` and the
/// record is a single content-free aggregate — the precise id/type live only in
/// the (non-exposed) `exposure_decision` audit (R11 #9 existence-leak fix). For
/// benign exclusions the caller is otherwise allowed to see (budget, redaction,
/// not-current), the id/type are retained.
#[derive(Debug, Clone)]
pub struct ExclusionRecord {
    pub object_type: Option<String>,
    pub object_id: Option<String>,
    pub reason: String, // coarse code from DenyReason or "budget_exhausted"
}

/// The compiled packet (the ephemeral body; the audit lives in exposure_decisions).
#[derive(Debug, Clone)]
pub struct ContextPacket {
    pub items: Vec<ContextPacketItem>,
    pub excluded: Vec<ExclusionRecord>,
    pub tokens_used: usize,
    pub truncated: bool,
}

impl ContextPacket {
    pub fn includes(&self, id: &str) -> bool {
        self.items.iter().any(|i| i.object_id == id)
    }
    pub fn exclusion_reason(&self, id: &str) -> Option<&str> {
        self.excluded
            .iter()
            .find(|e| e.object_id.as_deref() == Some(id))
            .map(|e| e.reason.as_str())
    }
}

pub struct PacketCompiler;

impl PacketCompiler {
    /// Compile a packet from candidates. `now` is passed in (no `Utc::now()`
    /// inside) so the compile is deterministic/testable.
    pub fn compile(
        candidates: &[PacketCandidate],
        request: &PacketRequest,
        now: DateTime<Utc>,
    ) -> ContextPacket {
        // Survivors carry their raw per-signal components; the fused score is
        // computed in a SECOND pass (bm25 rank-normalization is pool-relative, so
        // it can only run once the full survivor set is known).
        let mut survivors: Vec<(&PacketCandidate, RelevanceParts)> = Vec::new();
        let mut excluded: Vec<ExclusionRecord> = Vec::new();

        // ---- Layer A: hard gates (ExposureGate) ----
        for c in candidates {
            match ExposureGate::decide(&c.envelope, &c.redaction_status, &request.exposure) {
                crate::safety::ExposureDecision::Allow => {
                    survivors.push((c, Self::relevance_parts(c, request, now)));
                }
                crate::safety::ExposureDecision::Deny(reason) => {
                    // Over-ceiling / out-of-scope must NOT reveal the hidden item's
                    // id/type/count (existence leak, R11 #9) — emit ONE content-free
                    // aggregate per reason. Benign exclusions keep their handle.
                    let leaks_existence = matches!(
                        reason,
                        DenyReason::OverSensitivityCeiling | DenyReason::OutOfScope
                    );
                    let code = reason.code().to_string();
                    if leaks_existence {
                        if !excluded
                            .iter()
                            .any(|e| e.object_id.is_none() && e.reason == code)
                        {
                            excluded.push(ExclusionRecord {
                                object_type: None,
                                object_id: None,
                                reason: code,
                            });
                        }
                    } else {
                        excluded.push(ExclusionRecord {
                            object_type: Some(c.envelope.object_type.clone()),
                            object_id: Some(c.envelope.id.clone()),
                            reason: code,
                        });
                    }
                }
            }
        }

        // ---- Layer B: deterministic fusion of survivors ----
        // bm25 is rank-normalized over the survivor set (magnitude is unbounded
        // and version-unstable; the *ordering* is stable). The fused score and
        // its dominant `why.rule` are computed here, once the pool is known.
        let scored: Vec<(&PacketCandidate, f64, WhyIncluded)> =
            Self::fuse_survivors(&survivors);
        let mut survivors = scored;

        // sort by fused score desc, then object_id asc (total order, INV-6) — the
        // id tie-break guarantees byte-equal packets across runs/SQLite versions.
        survivors.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.envelope.id.cmp(&b.0.envelope.id))
        });

        // ---- token-budget packing (whole-item only, never mid-fact) ----
        let mut items = Vec::new();
        let mut tokens_used = 0usize;
        let mut truncated = false;
        for (rank, (c, score, why)) in survivors.into_iter().enumerate() {
            let tok = (c.title.len() / 4).max(1);
            if tokens_used + tok > request.token_budget && !items.is_empty() {
                excluded.push(ExclusionRecord {
                    // within ceiling+scope (a survivor) — id is safe to surface.
                    object_type: Some(c.envelope.object_type.clone()),
                    object_id: Some(c.envelope.id.clone()),
                    reason: "budget_exhausted".to_string(),
                });
                truncated = true;
                continue;
            }
            tokens_used += tok;
            items.push(ContextPacketItem {
                object_type: c.envelope.object_type.clone(),
                object_id: c.envelope.id.clone(),
                title: c.title.clone(),
                rank: rank + 1,
                sensitivity: c.envelope.sensitivity.clone(),
                fused_score: score,
                why,
                token_count: tok,
            });
        }

        ContextPacket {
            items,
            excluded,
            tokens_used,
            truncated,
        }
    }

    /// Per-candidate Layer-B signal components (everything that does NOT depend on
    /// the rest of the survivor pool). bm25 normalization is pool-relative, so the
    /// raw bm25 is just carried through here and resolved in [`Self::fuse_survivors`].
    fn relevance_parts(
        c: &PacketCandidate,
        request: &PacketRequest,
        now: DateTime<Utc>,
    ) -> RelevanceParts {
        // tag-match: fraction of query terms found in the candidate's categories/title.
        let hay: Vec<String> = c
            .categories
            .iter()
            .map(|s| s.to_lowercase())
            .chain(std::iter::once(c.title.to_lowercase()))
            .collect();
        let mut hits = 0usize;
        for term in &request.query_terms {
            let t = term.to_lowercase();
            if hay.iter().any(|h| h.contains(&t)) {
                hits += 1;
            }
        }
        let s_tag = if request.query_terms.is_empty() {
            0.0
        } else {
            hits as f64 / request.query_terms.len() as f64
        };

        // recency: 0.5 ^ (age_days / 14)
        let age_days = (now - c.envelope.updated_at).num_seconds().max(0) as f64 / 86_400.0;
        let s_rec = 0.5_f64.powf(age_days / 14.0);

        // graph: caller-precomputed edge weight, squashed into [0,1) so it can't
        // dominate the fusion (a deterministic, pool-independent transform).
        let s_graph = 1.0 - 0.5_f64.powf(c.graph_signal.max(0.0));

        RelevanceParts {
            tag_hits: hits,
            s_tag,
            s_rec,
            s_graph,
            bm25: c.bm25,
        }
    }

    /// Fuse the survivor pool into `(candidate, fused_score, why)`. bm25 is
    /// rank-normalized ACROSS survivors (best raw bm25 → 1.0, worst → 0.0, single
    /// survivor / no bm25 → 0.0 component) so the fused score is independent of the
    /// absolute bm25 magnitude — byte-equal across SQLite versions. The `why.rule`
    /// names the dominant contributing signal (deterministic precedence on ties).
    fn fuse_survivors<'c>(
        survivors: &[(&'c PacketCandidate, RelevanceParts)],
    ) -> Vec<(&'c PacketCandidate, f64, WhyIncluded)> {
        // Rank-normalize bm25 over the survivors that actually have a score.
        // SQLite bm25: lower (more negative) = better. We rank best→worst with a
        // stable id tie-break, then map rank to [0,1] (best=1.0). Determinism: the
        // mapping depends only on the bm25 ORDERING (+ id), never the magnitude.
        let mut bm25_ranked: Vec<&str> = survivors
            .iter()
            .filter(|(_, p)| p.bm25.is_some())
            .map(|(c, _)| c.envelope.id.as_str())
            .collect();
        bm25_ranked.sort_by(|a, b| {
            let pa = survivors.iter().find(|(c, _)| c.envelope.id == *a).unwrap().1.bm25.unwrap();
            let pb = survivors.iter().find(|(c, _)| c.envelope.id == *b).unwrap().1.bm25.unwrap();
            // lower bm25 first (better), id asc on ties → total order.
            pa.partial_cmp(&pb)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.cmp(b))
        });
        let n = bm25_ranked.len();

        survivors
            .iter()
            .map(|(c, p)| {
                let s_bm25 = if p.bm25.is_some() && n > 1 {
                    let rank = bm25_ranked
                        .iter()
                        .position(|id| *id == c.envelope.id.as_str())
                        .unwrap();
                    // best (rank 0) → 1.0, worst (rank n-1) → 0.0.
                    1.0 - (rank as f64 / (n - 1) as f64)
                } else {
                    // single bm25 survivor or none: no discriminating bm25 signal.
                    0.0
                };

                let fused = W_BM25 * s_bm25
                    + W_TAG * p.s_tag
                    + W_GRAPH * p.s_graph
                    + W_RECENCY * p.s_rec;

                // Dominant signal for the `why` breadcrumb (deterministic precedence
                // bm25 → tag → graph → recency on equal contributions).
                let contrib = [
                    ("bm25", W_BM25 * s_bm25),
                    ("tag_match", W_TAG * p.s_tag),
                    ("graph", W_GRAPH * p.s_graph),
                    ("recency", W_RECENCY * p.s_rec),
                ];
                let rule = if p.tag_hits == 0 && p.bm25.is_none() && p.s_graph == 0.0 {
                    // nothing matched the query — recency is all that ranks it.
                    "recency"
                } else {
                    contrib
                        .iter()
                        .max_by(|a, b| {
                            a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)
                        })
                        .map(|(name, _)| *name)
                        .unwrap_or("recency")
                };

                (
                    *c,
                    fused,
                    WhyIncluded {
                        rule: rule.to_string(),
                        fused_score: fused,
                    },
                )
            })
            .collect()
    }
}

/// Per-candidate signal components (pre-fusion). bm25 is carried raw; its
/// normalization is pool-relative and happens in [`PacketCompiler::fuse_survivors`].
struct RelevanceParts {
    tag_hits: usize,
    s_tag: f64,
    s_rec: f64,
    s_graph: f64,
    bm25: Option<f64>,
}

// R12 fusion weights (constants for now — versioned profiles are a later task).
// tag-match stays dominant so existing tag-only behavior is preserved when bm25
// and graph are absent (`bm25=None`, `graph=0` → fused = 0.45*tag + 0.15*rec,
// same ordering as the old 0.7*tag + 0.3*rec).
const W_BM25: f64 = 0.25;
const W_TAG: f64 = 0.45;
const W_GRAPH: f64 = 0.15;
const W_RECENCY: f64 = 0.15;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Domain;
    use crate::envelope::{Provenance, ProvenanceOrigin};

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn cand(
        id: &str,
        domain: Domain,
        sens: Sensitivity,
        cats: &[&str],
        title: &str,
    ) -> PacketCandidate {
        let mut e = Envelope::new(
            id,
            "decision",
            now(),
            Provenance::new(ProvenanceOrigin::PavleDirect),
        );
        e.domain = domain;
        e.sensitivity = sens;
        PacketCandidate::new(
            e,
            title,
            cats.iter().map(|s| s.to_string()).collect(),
            RedactionStatus::Clean,
        )
    }

    fn work_request(terms: &[&str]) -> PacketRequest {
        PacketRequest {
            intent: "task_work".into(),
            project: Some("altevra".into()),
            query_terms: terms.iter().map(|s| s.to_string()).collect(),
            exposure: ExposureRequest::default_work(),
            token_budget: 10_000,
        }
    }

    #[test]
    fn health_object_excluded_business_included() {
        let cands = vec![
            cand(
                "d1",
                Domain::Project,
                Sensitivity::Internal,
                &["storage"],
                "SQLite decision",
            ),
            cand(
                "h1",
                Domain::Health,
                Sensitivity::Restricted,
                &["sleep"],
                "Sleep pattern",
            ),
        ];
        let pkt = PacketCompiler::compile(&cands, &work_request(&["storage"]), now());
        assert!(pkt.includes("d1"));
        assert!(!pkt.includes("h1"));
        // Existence not leaked: the over-ceiling item has NO per-id handle in the
        // packet (R11 #9) — a caller cannot probe by id to confirm it exists.
        assert_eq!(pkt.exclusion_reason("h1"), None);
        // Only a single content-free aggregate notice is present.
        assert!(pkt
            .excluded
            .iter()
            .any(|e| e.reason == "items_above_ceiling_omitted" && e.object_id.is_none()));
        assert!(pkt
            .excluded
            .iter()
            .all(|e| e.object_id.as_deref() != Some("h1")));
    }

    #[test]
    fn unscanned_candidate_excluded_redaction_insufficient() {
        // R11 #8: an unscanned object within ceiling+scope must be denied at the
        // redaction gate, not exposed (the gate used to receive None and skip it).
        let mut c = cand(
            "u1",
            Domain::Business,
            Sensitivity::Internal,
            &["x"],
            "Unscanned note",
        );
        c.redaction_status = RedactionStatus::Unscanned;
        let pkt = PacketCompiler::compile(&[c], &work_request(&["x"]), now());
        assert!(!pkt.includes("u1"));
        // within ceiling+scope, so the id may be surfaced (not an existence leak).
        assert_eq!(pkt.exclusion_reason("u1"), Some("redaction_insufficient"));
    }

    #[test]
    fn over_ceiling_and_superseded_item_does_not_leak_id() {
        // R11 re-verify: a Restricted item that is ALSO superseded must still be
        // denied for the CEILING (content-free aggregate), not NotCurrent (which
        // would surface its id/type). Gate now checks ceiling before lifecycle.
        let mut c = cand(
            "h_old",
            Domain::Health,
            Sensitivity::Restricted,
            &["x"],
            "old health note",
        );
        c.envelope.status = crate::status::ObjectStatus::Superseded;
        let pkt = PacketCompiler::compile(&[c], &work_request(&["x"]), now());
        assert!(!pkt.includes("h_old"));
        assert_eq!(
            pkt.exclusion_reason("h_old"),
            None,
            "id leaked via NotCurrent"
        );
        assert!(pkt
            .excluded
            .iter()
            .all(|e| e.object_id.as_deref() != Some("h_old")));
    }

    #[test]
    fn tag_match_ranks_above_unrelated() {
        let cands = vec![
            cand(
                "a",
                Domain::Business,
                Sensitivity::Internal,
                &["gtm"],
                "GTM plan",
            ),
            cand(
                "b",
                Domain::Business,
                Sensitivity::Internal,
                &["storage"],
                "DB notes",
            ),
        ];
        let pkt = PacketCompiler::compile(&cands, &work_request(&["gtm"]), now());
        assert_eq!(pkt.items[0].object_id, "a");
        assert_eq!(pkt.items[0].why.rule, "tag_match");
    }

    #[test]
    fn deterministic_compile() {
        let cands = vec![
            cand("a", Domain::Business, Sensitivity::Internal, &["x"], "A"),
            cand("b", Domain::Business, Sensitivity::Internal, &["x"], "B"),
        ];
        let r = work_request(&["x"]);
        let p1 = PacketCompiler::compile(&cands, &r, now());
        let p2 = PacketCompiler::compile(&cands, &r, now());
        let ids1: Vec<_> = p1.items.iter().map(|i| &i.object_id).collect();
        let ids2: Vec<_> = p2.items.iter().map(|i| &i.object_id).collect();
        assert_eq!(ids1, ids2);
    }

    #[test]
    fn budget_truncates_with_record() {
        let cands = vec![
            cand(
                "a",
                Domain::Business,
                Sensitivity::Internal,
                &["x"],
                "A title that is reasonably long",
            ),
            cand(
                "b",
                Domain::Business,
                Sensitivity::Internal,
                &["x"],
                "B title that is reasonably long",
            ),
        ];
        let mut r = work_request(&["x"]);
        r.token_budget = 1; // only first item fits
        let pkt = PacketCompiler::compile(&cands, &r, now());
        assert_eq!(pkt.items.len(), 1);
        assert!(pkt.truncated);
        assert!(pkt.excluded.iter().any(|e| e.reason == "budget_exhausted"));
    }

    #[test]
    fn packet_bm25_graph_fusion() {
        // Three survivors, identical tag-match + recency, so ONLY the bm25 + graph
        // signals decide the order. bm25 is raw SQLite (lower = better).
        //  - "best_bm25": strongest bm25, no graph edge
        //  - "graphed":   weak bm25, strong graph signal
        //  - "weak":      worst bm25, no graph
        let best = cand("best_bm25", Domain::Business, Sensitivity::Internal, &["x"], "X one")
            .with_bm25(-9.0);
        let graphed = cand("graphed", Domain::Business, Sensitivity::Internal, &["x"], "X two")
            .with_bm25(-1.0)
            .with_graph_signal(1.0);
        let weak = cand("weak", Domain::Business, Sensitivity::Internal, &["x"], "X three")
            .with_bm25(-0.5);
        let cands = vec![weak, graphed, best];

        let pkt = PacketCompiler::compile(&cands, &work_request(&["x"]), now());
        let order: Vec<&str> = pkt.items.iter().map(|i| i.object_id.as_str()).collect();
        // tag-match + recency are identical across the three; bm25 (then graph) is
        // the ONLY discriminator: best bm25 wins, the graphed item (mid bm25 + graph
        // boost) beats weak. This proves bm25/graph actually move the ranking.
        assert_eq!(order, vec!["best_bm25", "graphed", "weak"]);
        // fused scores are strictly descending (bm25 rank-norm + graph spread them).
        assert!(pkt.items[0].fused_score > pkt.items[1].fused_score);
        assert!(pkt.items[1].fused_score > pkt.items[2].fused_score);
        // every survivor matched the query, so none is ranked by recency alone.
        assert!(pkt.items.iter().all(|i| i.why.rule != "recency"));

        // Determinism: same input → byte-equal packet (ids, ranks, scores, why).
        let p1 = PacketCompiler::compile(&cands, &work_request(&["x"]), now());
        let p2 = PacketCompiler::compile(&cands, &work_request(&["x"]), now());
        let snap = |p: &ContextPacket| -> Vec<(String, usize, u64, String)> {
            p.items
                .iter()
                .map(|i| {
                    (
                        i.object_id.clone(),
                        i.rank,
                        i.fused_score.to_bits(),
                        i.why.rule.clone(),
                    )
                })
                .collect()
        };
        assert_eq!(snap(&p1), snap(&p2), "fusion must be byte-equal across runs");

        // No-bm25 fallback: clearing the bm25 signals must not panic and must keep
        // the old tag+recency ordering (graph still applies). With equal tags here,
        // the graphed item floats to the top on graph alone.
        let no_bm25: Vec<PacketCandidate> = cands
            .iter()
            .cloned()
            .map(|mut c| {
                c.bm25 = None;
                c
            })
            .collect();
        let pkt2 = PacketCompiler::compile(&no_bm25, &work_request(&["x"]), now());
        assert_eq!(pkt2.items[0].object_id, "graphed");

        // When the query terms don't hit any tag/title (s_tag = 0), the dominant
        // `why.rule` falls to the bm25 / graph signal rather than recency — proving
        // those signals are first-class in the explanation, not just the score.
        let pkt3 = PacketCompiler::compile(&cands, &work_request(&["zzz_no_tag_hit"]), now());
        let best_item = pkt3.items.iter().find(|i| i.object_id == "best_bm25").unwrap();
        // best bm25 (rank-norm 1.0 → 0.25) outweighs recency (0.15) → explained as bm25.
        assert_eq!(best_item.why.rule, "bm25");
        // and bm25 still orders the pool even with zero tag-match.
        assert_eq!(pkt3.items[0].object_id, "best_bm25");
    }
}

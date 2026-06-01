//! Context Packet Compiler (working draft §3, RECONCILIATION R12).
//!
//! Turns broad capture into precise, source-backed, sensitivity-safe context.
//! Two strictly separated layers (the core anti-RAG-soup principle):
//!
//!  - **Layer A — gates** (hard, boolean): the [`ExposureGate`] decides
//!    inclusion by scope/sensitivity/lifecycle/redaction. A gate failure is
//!    NEVER overridden by relevance.
//!  - **Layer B — relevance** (soft): ranks SURVIVORS only, by
//!    `tag_match + recency` (NO vectors, R12 — deterministic, no model).
//!
//! Every item carries a resolvable `object_ref`; every inclusion AND exclusion
//! is explained. Deterministic given the same candidates + request.

use crate::envelope::Envelope;
use crate::safety::{ExposureGate, ExposureRequest};
use crate::security::Sensitivity;
use chrono::{DateTime, Utc};

/// A candidate object for packet compilation (from `object_index`).
#[derive(Debug, Clone)]
pub struct PacketCandidate {
    pub envelope: Envelope,
    pub title: String,
    /// Governed categories + tags used for the tag-match relevance signal.
    pub categories: Vec<String>,
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
    pub rule: String, // "tag_match" | "recency" | "structured"
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
#[derive(Debug, Clone)]
pub struct ExclusionRecord {
    pub object_type: String,
    pub object_id: String,
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
            .find(|e| e.object_id == id)
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
        let mut survivors: Vec<(&PacketCandidate, f64, WhyIncluded)> = Vec::new();
        let mut excluded: Vec<ExclusionRecord> = Vec::new();

        // ---- Layer A: hard gates (ExposureGate) ----
        for c in candidates {
            match ExposureGate::decide(&c.envelope, None, &request.exposure) {
                crate::safety::ExposureDecision::Allow => {
                    let (score, why) = Self::relevance(c, request, now);
                    survivors.push((c, score, why));
                }
                crate::safety::ExposureDecision::Deny(reason) => {
                    excluded.push(ExclusionRecord {
                        object_type: c.envelope.object_type.clone(),
                        object_id: c.envelope.id.clone(),
                        reason: reason.code().to_string(),
                    });
                }
            }
        }

        // ---- Layer B: deterministic ranking of survivors ----
        // sort by fused score desc, then object_id asc (total order, INV-6).
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
                    object_type: c.envelope.object_type.clone(),
                    object_id: c.envelope.id.clone(),
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

    /// Layer-B relevance: tag-match + recency (NO vectors, R12). Pool-independent
    /// normalizers so the score is deterministic regardless of candidate set.
    fn relevance(
        c: &PacketCandidate,
        request: &PacketRequest,
        now: DateTime<Utc>,
    ) -> (f64, WhyIncluded) {
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

        // fuse (tag dominant; recency tiebreaker). Weights are config in P0.4.
        let fused = 0.7 * s_tag + 0.3 * s_rec;
        let rule = if hits > 0 { "tag_match" } else { "recency" };
        (
            fused,
            WhyIncluded {
                rule: rule.to_string(),
                fused_score: fused,
            },
        )
    }
}

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
        PacketCandidate {
            envelope: e,
            title: title.to_string(),
            categories: cats.iter().map(|s| s.to_string()).collect(),
        }
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
        assert_eq!(
            pkt.exclusion_reason("h1"),
            Some("items_above_ceiling_omitted")
        );
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
}

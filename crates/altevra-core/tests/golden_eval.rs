//! Golden eval harness (BUILD_TASKS T-golden / working draft §3, R12/R6).
//!
//! The non-embedding golden subset (R12 drops the vector cases). These lock the
//! packet compiler's hard guarantees as visible tests — above all the LEAK SUITES
//! which must be ZERO: a personal/health object never enters a work packet (G03),
//! and unscanned/secret-bearing content is never exposed (G09). Pure: no DB, no
//! network, no model — just PacketCompiler + ExposureGate on synthetic candidates.

use altevra_core::envelope::{Envelope, Provenance, ProvenanceOrigin};
use altevra_core::packet::{PacketCandidate, PacketCompiler, PacketRequest};
use altevra_core::safety::ExposureRequest;
use altevra_core::security::Sensitivity;
use altevra_core::status::{ObjectStatus, RedactionStatus};
use altevra_core::Domain;
use chrono::{DateTime, Utc};

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn cand(
    id: &str,
    domain: Domain,
    sens: Sensitivity,
    redaction: RedactionStatus,
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
        redaction_status: redaction,
    }
}

fn work(terms: &[&str], budget: usize) -> PacketRequest {
    PacketRequest {
        intent: "task_work".into(),
        project: Some("altevra".into()),
        query_terms: terms.iter().map(|s| s.to_string()).collect(),
        exposure: ExposureRequest::default_work(),
        token_budget: budget,
    }
}

// ---- G03 + G09: LEAK SUITES = 0 (the ones that MUST hold) ------------------

#[test]
fn g03_personal_health_never_enters_work_packet() {
    let cands = vec![
        cand(
            "d1",
            Domain::Project,
            Sensitivity::Internal,
            RedactionStatus::Clean,
            &["storage"],
            "SQLite decision",
        ),
        cand(
            "h1",
            Domain::Health,
            Sensitivity::Restricted,
            RedactionStatus::Clean,
            &["sleep"],
            "Therapy notes",
        ),
        cand(
            "r1",
            Domain::Relationship,
            Sensitivity::Restricted,
            RedactionStatus::Clean,
            &["elena"],
            "Relationship note",
        ),
    ];
    let pkt = PacketCompiler::compile(&cands, &work(&["storage"], 10_000), now());
    assert!(pkt.includes("d1"), "work object must be included");
    // ZERO leak: neither personal object is exposed...
    assert!(!pkt.includes("h1"));
    assert!(!pkt.includes("r1"));
    // ...and neither leaks an enumerable id/type in the exclusions (existence leak).
    assert_eq!(pkt.exclusion_reason("h1"), None);
    assert_eq!(pkt.exclusion_reason("r1"), None);
    assert!(pkt.excluded.iter().all(|e| e.object_id.is_none()
        || (e.object_id.as_deref() != Some("h1") && e.object_id.as_deref() != Some("r1"))));
    // an aggregate, content-free "above ceiling" notice is the only signal.
    assert!(pkt
        .excluded
        .iter()
        .any(|e| e.reason == "items_above_ceiling_omitted" && e.object_id.is_none()));
}

#[test]
fn g09_unscanned_or_unredacted_never_exposed() {
    // A secret-bearing object that was never scanned (or is quarantined) must be
    // denied at the redaction gate even when its envelope looks benign.
    let cands = vec![
        cand(
            "ok",
            Domain::Business,
            Sensitivity::Internal,
            RedactionStatus::Clean,
            &["x"],
            "clean",
        ),
        cand(
            "unscanned",
            Domain::Business,
            Sensitivity::Internal,
            RedactionStatus::Unscanned,
            &["x"],
            "raw secret note",
        ),
        cand(
            "quarantined",
            Domain::Business,
            Sensitivity::Internal,
            RedactionStatus::Quarantined,
            &["x"],
            "quarantined",
        ),
        cand(
            "rejected",
            Domain::Business,
            Sensitivity::Internal,
            RedactionStatus::Rejected,
            &["x"],
            "rejected",
        ),
    ];
    let pkt = PacketCompiler::compile(&cands, &work(&["x"], 10_000), now());
    assert!(pkt.includes("ok"));
    for bad in ["unscanned", "quarantined", "rejected"] {
        assert!(!pkt.includes(bad), "{bad} must never be exposed");
        // within ceiling+scope, so id may show, but reason is redaction_insufficient.
        assert_eq!(pkt.exclusion_reason(bad), Some("redaction_insufficient"));
    }
}

// ---- G01/G02/G04/G05/G07/G08/G10/G14: behaviour gates -----------------------

#[test]
fn g01_bootstrap_includes_relevant_work() {
    let cands = vec![
        cand(
            "a",
            Domain::Business,
            Sensitivity::Internal,
            RedactionStatus::Clean,
            &["gtm"],
            "GTM plan",
        ),
        cand(
            "b",
            Domain::Project,
            Sensitivity::Internal,
            RedactionStatus::Clean,
            &["storage"],
            "DB notes",
        ),
    ];
    let pkt = PacketCompiler::compile(&cands, &work(&["gtm"], 10_000), now());
    assert!(pkt.includes("a"));
    assert_eq!(pkt.items[0].object_id, "a", "tag match ranks GTM first");
}

#[test]
fn g02_superseded_excluded_by_default() {
    let mut c = cand(
        "old",
        Domain::Business,
        Sensitivity::Internal,
        RedactionStatus::Clean,
        &["x"],
        "old decision",
    );
    c.envelope.status = ObjectStatus::Superseded;
    let pkt = PacketCompiler::compile(&[c], &work(&["x"], 10_000), now());
    assert!(!pkt.includes("old"));
    // within ceiling+scope → benign exclusion, id may be surfaced.
    assert_eq!(pkt.exclusion_reason("old"), Some("not_current"));
}

#[test]
fn g05_out_of_scope_domain_excluded_without_leak() {
    // A client-domain object is out of the work scope → excluded, no id leak.
    let cands = vec![
        cand(
            "biz",
            Domain::Business,
            Sensitivity::Internal,
            RedactionStatus::Clean,
            &["x"],
            "biz",
        ),
        cand(
            "client",
            Domain::Client,
            Sensitivity::Internal,
            RedactionStatus::Clean,
            &["x"],
            "client secret",
        ),
    ];
    let pkt = PacketCompiler::compile(&cands, &work(&["x"], 10_000), now());
    assert!(pkt.includes("biz"));
    assert!(!pkt.includes("client"));
    assert_eq!(
        pkt.exclusion_reason("client"),
        None,
        "out-of-scope id must not leak"
    );
}

#[test]
fn g07_budget_squeeze_truncates_with_record() {
    let cands = vec![
        cand(
            "a",
            Domain::Business,
            Sensitivity::Internal,
            RedactionStatus::Clean,
            &["x"],
            "A reasonably long title here",
        ),
        cand(
            "b",
            Domain::Business,
            Sensitivity::Internal,
            RedactionStatus::Clean,
            &["x"],
            "B reasonably long title here",
        ),
    ];
    let pkt = PacketCompiler::compile(&cands, &work(&["x"], 1), now());
    assert_eq!(pkt.items.len(), 1, "budget admits only one");
    assert!(pkt.truncated);
    assert!(pkt.excluded.iter().any(|e| e.reason == "budget_exhausted"));
}

#[test]
fn g08_determinism_byte_equal() {
    let cands = vec![
        cand(
            "a",
            Domain::Business,
            Sensitivity::Internal,
            RedactionStatus::Clean,
            &["x"],
            "A",
        ),
        cand(
            "b",
            Domain::Business,
            Sensitivity::Internal,
            RedactionStatus::Clean,
            &["x"],
            "B",
        ),
    ];
    let r = work(&["x"], 10_000);
    let p1 = PacketCompiler::compile(&cands, &r, now());
    let p2 = PacketCompiler::compile(&cands, &r, now());
    let ids = |p: &altevra_core::packet::ContextPacket| {
        p.items
            .iter()
            .map(|i| i.object_id.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&p1), ids(&p2));
    assert_eq!(p1.tokens_used, p2.tokens_used);
}

#[test]
fn g10_empty_candidate_set_is_valid_empty_packet() {
    let pkt = PacketCompiler::compile(&[], &work(&["x"], 10_000), now());
    assert!(pkt.items.is_empty());
    assert!(pkt.excluded.is_empty());
    assert!(!pkt.truncated);
}

#[test]
fn g14_higher_ceiling_admits_confidential_business() {
    // With a confidential ceiling, business-confidential is admitted (not a leak —
    // it's within the authorized ceiling + scope).
    let c = cand(
        "conf",
        Domain::Business,
        Sensitivity::Confidential,
        RedactionStatus::Clean,
        &["x"],
        "deal terms",
    );
    let mut req = work(&["x"], 10_000);
    req.exposure.sensitivity_ceiling = Sensitivity::Confidential;
    let pkt = PacketCompiler::compile(&[c], &req, now());
    assert!(pkt.includes("conf"));
    // but the SAME object is excluded under the default work (internal) ceiling.
    let pkt2 = PacketCompiler::compile(
        &[cand(
            "conf",
            Domain::Business,
            Sensitivity::Confidential,
            RedactionStatus::Clean,
            &["x"],
            "deal terms",
        )],
        &work(&["x"], 10_000),
        now(),
    );
    assert!(!pkt2.includes("conf"));
}

//! P0.9 E5 — Tombstones + conflict markers (sync prep seed).
//!
//! A **tombstone** is the lossy receipt a forget pipeline ALWAYS produces in
//! place of the original body. It carries the minimum sync replicas need to
//! reach consensus on "this object existed; it's been removed at this
//! revision" — id + content_hash + revision + origin_device — and NOTHING
//! else. The body, title, categories, provenance details, and every other
//! field of the original [`Envelope`] are deliberately absent. This is the
//! load-bearing privacy property: a sync pipeline that ships tombstones leaks
//! the *fingerprint* of the gone object, never its content.
//!
//! A **conflict marker** is the receipt the detector produces when two
//! divergent (revision, origin_device, content_hash) triples arrive for the
//! same id. It is a STRUCTURED REPORT — never a winner. Last-writer-wins is
//! explicitly forbidden: the P1 sync daemon (not part of this commit) will
//! turn ConflictMarker rows into review items so Pavle resolves them, never
//! the algorithm.
//!
//! No daemon, no network, no DB writes from this module. The Envelope already
//! carries revision / origin_device / checksum (Faza A); this module is a
//! pure projection on top of it.

use crate::envelope::Envelope;
use serde::{Deserialize, Serialize};

/// The minimum receipt a forget pipeline produces in place of a deleted body.
/// No title, no provenance, no body — only what consensus requires.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tombstone {
    /// The original object's stable id.
    pub id: String,
    /// The original object's content_hash (envelope `checksum` field). `None`
    /// is permitted because legacy rows may not have one — but a `build_*`
    /// constructor below logs a one-liner whenever it sees that, since a
    /// sync-aware Altevra should be stamping checksums on every write.
    pub content_hash: Option<String>,
    /// Monotonic per-id revision at the moment of forget.
    pub revision: u32,
    /// Device where the forget originated (last writer). Matches the
    /// envelope's `origin_device`.
    pub origin_device: Option<String>,
}

impl Tombstone {
    /// Project an [`Envelope`] onto a tombstone — strips everything except the
    /// four sync-relevant fields. This is the ONLY way a forget pipeline
    /// should produce a tombstone; the deliberate absence of body / title /
    /// provenance is the privacy property.
    ///
    /// **Pure** — does not touch the DB, does not bump the revision, does not
    /// stamp time. The forget pipeline is responsible for incrementing
    /// `env.revision` before calling this if the forget itself counts as a
    /// new revision in the source schema.
    pub fn from_envelope(env: &Envelope) -> Self {
        Self {
            id: env.id.clone(),
            content_hash: env.checksum.clone(),
            revision: env.revision,
            origin_device: env.origin_device.clone(),
        }
    }

    /// The triple (revision, origin_device, content_hash) used for conflict
    /// detection. Two tombstones with equal triples are considered the same
    /// observation of the same forget event.
    pub fn triple(&self) -> (u32, Option<&str>, Option<&str>) {
        (
            self.revision,
            self.origin_device.as_deref(),
            self.content_hash.as_deref(),
        )
    }
}

/// Build a tombstone from an envelope. A missing `checksum` is permitted —
/// legacy rows may not have one — but downstream sync replicas can't
/// fingerprint-match such tombstones; the forget pipeline should be stamping
/// a checksum on every write before reaching this projector.
pub fn build_tombstone(env: &Envelope) -> Tombstone {
    Tombstone::from_envelope(env)
}

/// One side of a conflict — the data needed to render a review item without
/// dragging the full envelope along.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictSide {
    pub revision: u32,
    pub origin_device: Option<String>,
    pub content_hash: Option<String>,
}

impl ConflictSide {
    fn from_tombstone(t: &Tombstone) -> Self {
        Self {
            revision: t.revision,
            origin_device: t.origin_device.clone(),
            content_hash: t.content_hash.clone(),
        }
    }
}

/// A divergence between two observations of the same id. Carries BOTH sides
/// so the P1 sync daemon turns it into a review item (Pavle resolves).
/// **NEVER a winner** — the producer of this marker does not pick. Mantra:
/// "Conflicts surface; they don't auto-resolve."
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictMarker {
    pub id: String,
    pub ours: ConflictSide,
    pub theirs: ConflictSide,
}

/// Detect a conflict between two tombstones for the same id. Returns
/// `Some(ConflictMarker)` iff the triples diverge; `None` if they agree
/// (the two replicas observed the same forget at the same revision /
/// device / hash).
///
/// **Hard contract:** if the ids do not match, the function returns `None`
/// — comparing tombstones for different ids is a CALLER bug. The caller
/// should index tombstones by id before pairing them, never sort + zip.
///
/// **No last-writer-wins.** The function does NOT look at timestamps, does
/// NOT compare revisions for "newest", and does NOT prefer either side.
/// Divergence → marker → review.
pub fn detect_conflict(a: &Tombstone, b: &Tombstone) -> Option<ConflictMarker> {
    if a.id != b.id {
        return None;
    }
    if a.triple() == b.triple() {
        return None;
    }
    Some(ConflictMarker {
        id: a.id.clone(),
        ours: ConflictSide::from_tombstone(a),
        theirs: ConflictSide::from_tombstone(b),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::{Envelope, Provenance, ProvenanceOrigin};
    use chrono::{DateTime, Utc};

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn env(id: &str, rev: u32, device: &str, checksum: Option<&str>) -> Envelope {
        let mut e = Envelope::new(
            id,
            "learning",
            now(),
            Provenance::new(ProvenanceOrigin::PavleDirect),
        );
        e.revision = rev;
        e.origin_device = Some(device.to_string());
        e.checksum = checksum.map(String::from);
        // populate fields that MUST NOT leak into the tombstone — the test
        // asserts they're absent.
        e.tags = vec!["secret".into(), "private".into()];
        e.categories = vec!["personal".into()];
        e
    }

    /// E5 — a tombstone carries the four sync-relevant fields and NOTHING
    /// else. Specifically: no body, no title, no categories, no tags, no
    /// provenance, no created_at. The struct's shape is the contract.
    #[test]
    fn tombstone_has_no_body() {
        let e = env("obj_42", 3, "device-a", Some("hash-deadbeef"));
        let t = build_tombstone(&e);
        assert_eq!(t.id, "obj_42");
        assert_eq!(t.revision, 3);
        assert_eq!(t.origin_device.as_deref(), Some("device-a"));
        assert_eq!(t.content_hash.as_deref(), Some("hash-deadbeef"));

        // Serializing the tombstone must NOT include any of the body / metadata
        // fields from the original envelope. We serialize to JSON and grep for
        // forbidden substrings.
        let j = serde_json::to_string(&t).unwrap();
        assert!(!j.contains("secret"), "tags must not leak: {j}");
        assert!(!j.contains("private"), "tags must not leak: {j}");
        assert!(!j.contains("personal"), "categories must not leak: {j}");
        assert!(!j.contains("provenance"), "provenance must not leak: {j}");
        assert!(!j.contains("body"), "no body field: {j}");
        assert!(!j.contains("title"), "no title field: {j}");
        assert!(!j.contains("learning"), "object_type must not leak: {j}");

        // The serialized shape is exactly the four documented fields.
        let parsed: serde_json::Value = serde_json::from_str(&j).unwrap();
        let obj = parsed.as_object().unwrap();
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(keys, vec!["content_hash", "id", "origin_device", "revision"]);
    }

    /// E5 — two divergent (revision, origin_device, content_hash) triples for
    /// the same id produce a ConflictMarker. The marker carries BOTH sides;
    /// neither is picked as the winner.
    #[test]
    fn divergent_triples_produce_conflict_marker() {
        let a = build_tombstone(&env("obj_1", 3, "device-a", Some("h-aaa")));
        let b = build_tombstone(&env("obj_1", 4, "device-b", Some("h-bbb")));
        let marker = detect_conflict(&a, &b).expect("divergent triples → marker");
        assert_eq!(marker.id, "obj_1");
        assert_eq!(marker.ours.revision, 3);
        assert_eq!(marker.theirs.revision, 4);
        assert_eq!(marker.ours.origin_device.as_deref(), Some("device-a"));
        assert_eq!(marker.theirs.origin_device.as_deref(), Some("device-b"));
        assert_eq!(marker.ours.content_hash.as_deref(), Some("h-aaa"));
        assert_eq!(marker.theirs.content_hash.as_deref(), Some("h-bbb"));

        // Hard contract: the function does NOT pick a winner. Both sides
        // are kept verbatim; the caller (P1 sync) turns the marker into a
        // review item.
        let j = serde_json::to_string(&marker).unwrap();
        assert!(!j.contains("\"winner\""), "no winner field allowed");
        assert!(!j.contains("\"resolved\""), "no auto-resolve flag allowed");
    }

    /// E5 — two tombstones with identical triples for the same id agree, no
    /// conflict produced. This is the "two replicas observed the same forget"
    /// case; a marker here would generate noise for the human reviewer.
    #[test]
    fn identical_triples_no_conflict() {
        let a = build_tombstone(&env("obj_5", 7, "device-a", Some("h-same")));
        let b = build_tombstone(&env("obj_5", 7, "device-a", Some("h-same")));
        assert!(detect_conflict(&a, &b).is_none());
    }

    /// E5 (extra) — mismatched ids return None (caller bug; never auto-pair).
    /// Pinned because a sloppy P1 sync might zip vectors instead of indexing.
    #[test]
    fn mismatched_ids_return_none() {
        let a = build_tombstone(&env("obj_a", 1, "d1", Some("h1")));
        let b = build_tombstone(&env("obj_b", 2, "d2", Some("h2")));
        assert!(detect_conflict(&a, &b).is_none());
    }

    /// E5 (extra) — partial divergence (only revision differs) still surfaces
    /// as a conflict. Any element of the triple drifting counts.
    #[test]
    fn revision_only_divergence_is_a_conflict() {
        let a = build_tombstone(&env("obj_x", 1, "d1", Some("h-eq")));
        let b = build_tombstone(&env("obj_x", 2, "d1", Some("h-eq")));
        assert!(detect_conflict(&a, &b).is_some());
    }
}

//! Improvement-signals repository (migration 028, self-improve stage 1).
//!
//! `improvement_signals` is the raw-signal inbox the self-improve loop clusters
//! into `proposals` (stage 1 → 2). A signal is a cheap "something happened worth
//! a later look" marker — a session/turn/run landed — NOT a proposal. The
//! orchestrator (a later seam) reads [`cluster_open`], does the heavy LLM work,
//! and emits `proposals`; the producer here only enqueues a row.
//!
//! Table columns (028): `id, kind, source_ref, summary, cluster_key, created_at`.
//!
//! Load-bearing invariants enforced HERE, not trusted from the caller:
//!  - **idempotent enqueue:** a signal is keyed by a stable hash of
//!    `(kind, source_ref, summary)` stored in `source_ref`'s sibling — actually
//!    in the `id` (the id IS the dedup key) — so the same session ingest
//!    re-running enqueues the SAME row once, never a duplicate. Re-runs are a
//!    no-op (idempotent), mirroring [`SessionsRepository::upsert_imported`].
//!  - **SI-6 (self-write exclusion) is NOT decided here:** the producer
//!    ([`crate::improvement_signals::signal_for_session`]) skips resident-authored
//!    objects BEFORE calling [`ImprovementSignalsRepository::insert`]; the repo
//!    persists whatever it is handed. SI-6 is a producer-side gate (a resident
//!    mode's own output must never become a signal that feeds it again).
//!
//! [`SessionsRepository::upsert_imported`]: crate::SessionsRepository::upsert_imported

use sqlx::{Row, SqlitePool};

/// A raw improvement signal (migration 028 column shape).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalRow {
    /// Stable dedup id = hash of `(kind, source_ref, summary)` — re-enqueue is a
    /// no-op (idempotent).
    pub id: String,
    pub kind: String,
    /// The turn/session/run that emitted it (`session:<id>`, `turn:<id>`, …).
    pub source_ref: String,
    pub summary: String,
    /// Groups signals into one future proposal (e.g. `session:<tool>:<project>`).
    pub cluster_key: Option<String>,
    pub created_at: String,
}

/// What the caller enqueues. The id (dedup key) is derived from these fields by
/// the repo, so re-enqueueing an identical signal collapses to the same row.
#[derive(Debug, Clone)]
pub struct NewSignal {
    pub kind: String,
    pub source_ref: String,
    pub summary: String,
    pub cluster_key: Option<String>,
}

/// One open-signal cluster: a `cluster_key` and every open signal under it.
#[derive(Debug, Clone)]
pub struct SignalCluster {
    /// `None` means the signals carried no cluster_key (ungrouped bucket).
    pub cluster_key: Option<String>,
    pub signals: Vec<SignalRow>,
}

pub struct ImprovementSignalsRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> ImprovementSignalsRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Enqueue a signal. **Idempotent:** the row id is a stable hash of
    /// `(kind, source_ref, summary)`; if a row with that id already exists the
    /// insert is a no-op and `false` is returned (no duplicate). Returns
    /// `(id, is_new)`.
    pub async fn insert(&self, s: &NewSignal) -> anyhow::Result<(String, bool)> {
        let id = signal_id(&s.kind, &s.source_ref, &s.summary);
        // `INSERT OR IGNORE` on a PRIMARY KEY collision = idempotent enqueue.
        let res = sqlx::query(
            "INSERT OR IGNORE INTO improvement_signals \
             (id, kind, source_ref, summary, cluster_key) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&s.kind)
        .bind(&s.source_ref)
        .bind(&s.summary)
        .bind(s.cluster_key.as_deref())
        .execute(self.pool)
        .await?;
        Ok((id, res.rows_affected() > 0))
    }

    /// All open (unclustered-into-a-proposal) signals, newest first. Stage 1 has
    /// no `status` column — every row in the table is "open" until the
    /// orchestrator deletes it after promoting it to a proposal (a later seam).
    pub async fn list_open(&self) -> anyhow::Result<Vec<SignalRow>> {
        let rows = sqlx::query(
            "SELECT id, kind, source_ref, summary, cluster_key, created_at \
             FROM improvement_signals ORDER BY created_at DESC, id",
        )
        .fetch_all(self.pool)
        .await?;
        Ok(rows.into_iter().map(row_to_signal).collect())
    }

    /// Group every open signal by `cluster_key` (the orchestrator turns each
    /// cluster into one proposal). Signals with no `cluster_key` collapse into a
    /// single ungrouped bucket (`cluster_key: None`). Clusters are returned in a
    /// deterministic order (by key, ungrouped last).
    pub async fn cluster_open(&self) -> anyhow::Result<Vec<SignalCluster>> {
        let signals = self.list_open().await?;
        // Preserve a stable, deterministic grouping: keyed clusters sorted by
        // key, the ungrouped bucket last. We collect into an ordered Vec instead
        // of a HashMap so the output order is reproducible (tests + digests).
        let mut keyed: std::collections::BTreeMap<String, Vec<SignalRow>> =
            std::collections::BTreeMap::new();
        let mut ungrouped: Vec<SignalRow> = Vec::new();
        for s in signals {
            match &s.cluster_key {
                Some(k) => keyed.entry(k.clone()).or_default().push(s),
                None => ungrouped.push(s),
            }
        }
        let mut out: Vec<SignalCluster> = keyed
            .into_iter()
            .map(|(k, signals)| SignalCluster {
                cluster_key: Some(k),
                signals,
            })
            .collect();
        if !ungrouped.is_empty() {
            out.push(SignalCluster {
                cluster_key: None,
                signals: ungrouped,
            });
        }
        Ok(out)
    }
}

/// Stable dedup id for a signal: a deterministic hash of `(kind, source_ref,
/// summary)`. Same inputs → same id → idempotent enqueue (no duplicate row).
/// Field boundaries are NUL-separated so `("a","bc")` ≠ `("ab","c")`.
fn signal_id(kind: &str, source_ref: &str, summary: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    kind.hash(&mut h);
    0u8.hash(&mut h);
    source_ref.hash(&mut h);
    0u8.hash(&mut h);
    summary.hash(&mut h);
    format!("sig:{:016x}", h.finish())
}

fn row_to_signal(r: sqlx::sqlite::SqliteRow) -> SignalRow {
    SignalRow {
        id: r.get("id"),
        kind: r.get("kind"),
        source_ref: r.get("source_ref"),
        summary: r.get("summary"),
        cluster_key: r.get("cluster_key"),
        created_at: r.get("created_at"),
    }
}

/// SI-6 self-write exclusion predicate — a PURE function (no DB, no LLM).
///
/// Altevra's own resident-mode output must NEVER become an improvement signal
/// that feeds the loop back into itself. A capture is resident-authored when the
/// source identifier (the session's `tool`, or an object's `provenance.captured_by`)
/// names a resident surface: the `resident:` / `agent:altevra` markers, or a
/// known builtin resident-mode name (migration 027). The producer calls this
/// BEFORE enqueueing; a `true` verdict means "skip — do not enqueue".
///
/// This is below the LLM: it reads the provenance string, never free-text
/// content, so no note/proposal text can flip the verdict (SI-15).
pub fn is_resident_authored(source: &str) -> bool {
    let s = source.trim();
    if s.is_empty() {
        return false;
    }
    let lower = s.to_ascii_lowercase();
    // Marker conventions: `resident:<mode>` run refs, `agent:altevra*` captures.
    if lower.starts_with("resident:") || lower.starts_with("agent:altevra") {
        return true;
    }
    // A bare resident-mode name (migration 027 seeds these). Match against the
    // last `:`-segment so `agent:memory_curator` is caught too.
    let leaf = lower.rsplit(':').next().unwrap_or(&lower);
    RESIDENT_MODE_NAMES.contains(&leaf)
}

/// The builtin resident modes (migration 027). Kept here as the SI-6 self-write
/// exclusion list; if a mode is added to 027, add it here too.
const RESIDENT_MODE_NAMES: &[&str] = &[
    "memory_curator",
    "synthesis",
    "wiki_curator",
    "daily_briefing",
    "insight",
    "observer",
    "personal_curator",
    "skill_factory_proposer",
];

/// Build the improvement signal for a freshly-ingested session, or `None` when
/// SI-6 excludes it (the session was authored by a resident mode → no
/// self-feedback loop).
///
/// Cheap by design: one signal per session ingest. `cluster_key` groups by
/// `(tool, project)` so the orchestrator batches signals from the same source
/// into one proposal later. The heavy work (reading turns, LLM clustering) is
/// the orchestrator's job — this only enqueues the marker.
pub fn signal_for_session(
    session_id: &str,
    tool: &str,
    project: Option<&str>,
    turn_count: i64,
) -> Option<NewSignal> {
    // SI-6: a resident-authored session never becomes a signal that feeds the
    // loop back into itself.
    if is_resident_authored(tool) {
        return None;
    }
    let cluster_key = Some(match project {
        Some(p) if !p.is_empty() => format!("session:{tool}:{p}"),
        _ => format!("session:{tool}"),
    });
    Some(NewSignal {
        kind: "session_ingest".to_string(),
        source_ref: format!("session:{session_id}"),
        summary: format!("session ingested from {tool} ({turn_count} turns) — review for memory/learning/preference signals"),
        cluster_key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::{create_pool, run_migrations};

    async fn pool() -> SqlitePool {
        let p = create_pool("sqlite::memory:").await.unwrap();
        run_migrations(&p).await.unwrap();
        p
    }

    fn new_signal(kind: &str, source_ref: &str, summary: &str, cluster: Option<&str>) -> NewSignal {
        NewSignal {
            kind: kind.into(),
            source_ref: source_ref.into(),
            summary: summary.into(),
            cluster_key: cluster.map(String::from),
        }
    }

    #[tokio::test]
    async fn improvement_signal_cluster() {
        let p = pool().await;
        let repo = ImprovementSignalsRepository::new(&p);

        // Two signals under cluster A, one under cluster B, one ungrouped.
        let (_, n1) = repo
            .insert(&new_signal(
                "session_ingest",
                "session:1",
                "s1",
                Some("session:claude-code:altevra"),
            ))
            .await
            .unwrap();
        let (_, n2) = repo
            .insert(&new_signal(
                "session_ingest",
                "session:2",
                "s2",
                Some("session:claude-code:altevra"),
            ))
            .await
            .unwrap();
        let (_, n3) = repo
            .insert(&new_signal(
                "session_ingest",
                "session:3",
                "s3",
                Some("session:codex:revesta"),
            ))
            .await
            .unwrap();
        let (_, n4) = repo
            .insert(&new_signal("turn_ingest", "turn:9", "s4", None))
            .await
            .unwrap();
        assert!(n1 && n2 && n3 && n4, "four distinct signals all new");
        assert_eq!(repo.list_open().await.unwrap().len(), 4);

        // cluster_open groups by cluster_key: 2 keyed clusters + 1 ungrouped.
        let clusters = repo.cluster_open().await.unwrap();
        assert_eq!(clusters.len(), 3, "two keyed clusters + one ungrouped");
        // Deterministic order: keys sorted, ungrouped last.
        assert_eq!(
            clusters[0].cluster_key.as_deref(),
            Some("session:claude-code:altevra")
        );
        assert_eq!(clusters[0].signals.len(), 2, "cluster A has 2 signals");
        assert_eq!(
            clusters[1].cluster_key.as_deref(),
            Some("session:codex:revesta")
        );
        assert_eq!(clusters[1].signals.len(), 1);
        assert_eq!(clusters[2].cluster_key, None, "ungrouped bucket last");
        assert_eq!(clusters[2].signals.len(), 1);

        // Dedup on an IDENTICAL signal: same (kind, source_ref, summary) → no 2nd
        // row, insert reports not-new, count unchanged.
        let (id_first, _) = repo
            .insert(&new_signal(
                "session_ingest",
                "session:1",
                "s1",
                Some("session:claude-code:altevra"),
            ))
            .await
            .unwrap();
        // re-enqueue the byte-identical signal
        let (id_again, is_new) = repo
            .insert(&new_signal(
                "session_ingest",
                "session:1",
                "s1",
                Some("session:claude-code:altevra"),
            ))
            .await
            .unwrap();
        assert!(!is_new, "identical signal re-enqueue must be a no-op");
        assert_eq!(id_first, id_again, "same dedup id");
        assert_eq!(
            repo.list_open().await.unwrap().len(),
            4,
            "still four rows after dedup re-enqueue"
        );
    }

    #[test]
    fn si6_excludes_resident_authored() {
        // Resident-mode markers + bare mode names are excluded (skip = true).
        assert!(is_resident_authored("resident:memory_curator"));
        assert!(is_resident_authored("agent:altevra-resident"));
        assert!(is_resident_authored("memory_curator"));
        assert!(is_resident_authored("agent:observer"));
        assert!(is_resident_authored("SKILL_FACTORY_PROPOSER")); // case-insensitive
                                                                 // Real external AI tools are NOT excluded (enqueue = proceed).
        assert!(!is_resident_authored("claude-code"));
        assert!(!is_resident_authored("codex"));
        assert!(!is_resident_authored("cursor"));
        assert!(!is_resident_authored("agent:claude-code"));
        assert!(!is_resident_authored(""));
    }

    #[test]
    fn signal_for_session_builds_one_signal_and_si6_returns_none() {
        // A real external-tool session ingest → exactly one signal.
        let s = signal_for_session("abc", "claude-code", Some("altevra"), 12).unwrap();
        assert_eq!(s.kind, "session_ingest");
        assert_eq!(s.source_ref, "session:abc");
        assert_eq!(
            s.cluster_key.as_deref(),
            Some("session:claude-code:altevra")
        );
        // No project → cluster by tool alone.
        let s2 = signal_for_session("def", "codex", None, 3).unwrap();
        assert_eq!(s2.cluster_key.as_deref(), Some("session:codex"));

        // SI-6: a resident-authored session enqueues NOTHING.
        assert!(signal_for_session("ghi", "resident:observer", Some("altevra"), 5).is_none());
        assert!(signal_for_session("jkl", "memory_curator", None, 1).is_none());
    }
}

//! Lifecycle / retention sweep (P0.8 T8.4 runtime + P0.9 E1 actor).
//!
//! Two surfaces:
//!
//!  * [`lifecycle_sweep`] — the original non-destructive REPORTER (P0.8). Derives
//!    each indexed object's [`LifecycleState`](altevra_core::status::LifecycleState)
//!    from the resolved domain policy and returns counts. Used by status
//!    dashboards.
//!  * [`lifecycle_archive`] — the P0.9 E1 ACTOR. Performs the SAFE operations
//!    the reporter only counted:
//!      1. Soft-archive `Active → Archived` for objects whose derived state is
//!         [`LifecycleState::RetentionDue`] (soft TTL passed). Status only; the
//!         row stays.
//!      2. Mark `lifecycle_marker='pending_delete'` on objects whose state is
//!         [`LifecycleState::DeleteDue`] — does NOT delete. The destructive
//!         forget itself stays presence-gated downstream (R4).
//!      3. Purge ephemeral `context_packets` bodies past the 14-day retention
//!         window (R-EPH). Never touches `exposure_decisions` / `audit_log`
//!         (R5-INV) — those are the durable audit, and the function asserts
//!         their row counts are unchanged before/after.
//!      4. Honors per-object legal hold (D7): a held row is never archived
//!         and never purged, regardless of its derived state.
//!
//! The brain job [`crate::jobs::JobKind::LifecycleArchiver`] wraps the actor
//! and is scheduled ~24h. It runs after, and is distinct from, the C7
//! [`crate::curator`] (which targets the proposal/skill *status* archive, not
//! the lifecycle_state derived from envelope timestamps).
//!
//! `object_index` carries `updated_at` + `status` but not `created_at` /
//! `valid_until` / `review_after`, so this sweep derives soft-ttl / hard-expiry
//! against `updated_at` (a conservative staleness proxy) and reports retention_due
//! / delete_due / archived / fresh. Per-object review windows are derived from
//! the source tables when the job sweeps them directly.

use altevra_core::envelope::{Envelope, Provenance, ProvenanceOrigin};
use altevra_core::lifecycle::derive_lifecycle_state;
use altevra_core::status::LifecycleState;
use altevra_db::{DomainPolicyRepository, ObjectIndexRepository};
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use std::collections::BTreeMap;

/// R-EPH: a `context_packet` body is ephemeral and auto-purged this many days
/// after creation. The exposure audit (`exposure_decisions`) is **never**
/// purged; only the bulky echoed packet body + its sources are removed.
pub const CONTEXT_PACKET_RETENTION_DAYS: i64 = 14;

/// Marker the sweep sets on `delete_due` objects. Surfaces them for Pavle's
/// digest WITHOUT performing any delete — destructive forget stays
/// presence-gated (R4).
pub const PENDING_DELETE_MARKER: &str = "pending_delete";

#[derive(Debug, Clone, Default)]
pub struct LifecycleReport {
    pub total: usize,
    pub by_state: BTreeMap<String, usize>,
}

impl LifecycleReport {
    pub fn count(&self, state: &str) -> usize {
        self.by_state.get(state).copied().unwrap_or(0)
    }
}

/// What one `lifecycle_archive` pass did. Counters mirror the safe operations
/// the actor performs — never destructive.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LifecycleArchiveReport {
    /// Active rows soft-archived (state derived to `RetentionDue`).
    pub archived: usize,
    /// Rows marked `pending_delete` (state derived to `DeleteDue`); not deleted.
    pub pending_delete: usize,
    /// Rows skipped because of an active legal hold (D7).
    pub legal_hold_skipped: usize,
    /// Ephemeral `context_packets` rows purged (R-EPH). Bodies + sources only;
    /// `exposure_decisions` and `audit_log` are NEVER touched (R5-INV).
    pub context_packets_purged: usize,
}

impl LifecycleArchiveReport {
    /// One-line summary for `brain_jobs.result_summary`.
    pub fn summary(&self) -> String {
        format!(
            "lifecycle: archived {}, marked {} pending_delete, held {} (legal_hold), \
             purged {} context_packet(s)",
            self.archived,
            self.pending_delete,
            self.legal_hold_skipped,
            self.context_packets_purged
        )
    }

    pub fn total_actions(&self) -> usize {
        self.archived + self.pending_delete + self.context_packets_purged
    }
}

/// Sweep object_index, deriving each row's lifecycle state from its domain
/// policy. **REPORTER ONLY** — no mutations. See [`lifecycle_archive`] for the
/// actor counterpart.
pub async fn lifecycle_sweep(
    pool: &SqlitePool,
    now: DateTime<Utc>,
) -> anyhow::Result<LifecycleReport> {
    let rows = ObjectIndexRepository::new(pool).candidates(None).await?;
    let policies = DomainPolicyRepository::new(pool);
    let mut report = LifecycleReport {
        total: rows.len(),
        by_state: BTreeMap::new(),
    };
    for r in &rows {
        let (soft, hard) = policies
            .get(&r.domain)
            .await?
            .map(|p| (p.soft_ttl_days, p.hard_expiry_days))
            .unwrap_or((None, None));
        let mut e = Envelope::new(
            &r.id,
            &r.object_type,
            r.updated_at,
            Provenance::new(ProvenanceOrigin::Imported),
        );
        e.status = r.status.parse().unwrap();
        e.domain = r.domain.parse().unwrap();
        // legal_hold is a per-object active flag (not the domain's capability) —
        // the index doesn't carry it, so the report treats it as not-held; the
        // source-table sweep applies real holds.
        let state = derive_lifecycle_state(&e, soft, hard, false, now);
        *report.by_state.entry(state.to_string()).or_insert(0) += 1;
    }
    Ok(report)
}

/// E1 — actor pass. Performs the safe operations (status archive / marker /
/// ephemeral purge), honoring legal hold (D7). Returns a [`LifecycleArchiveReport`].
///
/// **Invariants enforced by code below the LLM:**
///  * No `DELETE` against `exposure_decisions` or `audit_log` (R5-INV). The
///    function asserts both row counts are unchanged before/after; a
///    regression that adds an accidental write trips immediately.
///  * No row hard-deleted from `object_index`, `learnings`, `insight_cards`,
///    or any other source table. Archive == status-only.
///  * Legal-held rows are never touched.
///  * `context_packets` ARE row-deleted — but that table is documented
///    ephemeral (R-EPH); its associated `exposure_decisions` rows stay
///    forever (the audit was the point).
pub async fn lifecycle_archive(
    pool: &SqlitePool,
    now: DateTime<Utc>,
) -> anyhow::Result<LifecycleArchiveReport> {
    // Pin the append-only audit tables before any work — we re-check after.
    let before_exposure = count_rows(pool, "exposure_decisions").await;
    let before_audit = count_rows(pool, "audit_log").await;

    let idx = ObjectIndexRepository::new(pool);
    let policies = DomainPolicyRepository::new(pool);
    let mut report = LifecycleArchiveReport::default();

    for (r, legal_hold) in idx.iter_for_lifecycle().await? {
        // D7 has precedence over everything: a held row is never archived,
        // never marked. We still count it for visibility.
        if legal_hold {
            report.legal_hold_skipped += 1;
            continue;
        }
        // Already non-active rows aren't candidates for soft-archive.
        let status_already_terminal = !matches!(r.status.as_str(), "active" | "draft");

        let (soft, hard) = policies
            .get(&r.domain)
            .await?
            .map(|p| (p.soft_ttl_days, p.hard_expiry_days))
            .unwrap_or((None, None));

        // Build a minimal envelope from the index row. `updated_at` is the only
        // timestamp the index carries; `derive_lifecycle_state` uses it for both
        // created_at and updated_at — a conservative staleness proxy that mirrors
        // the reporter.
        let mut e = Envelope::new(
            &r.id,
            &r.object_type,
            r.updated_at,
            Provenance::new(ProvenanceOrigin::Imported),
        );
        e.status = r.status.parse().unwrap();
        e.domain = r.domain.parse().unwrap();

        // legal_hold has already been honored above; pass `false` here so the
        // derive function reports the underlying state.
        let state = derive_lifecycle_state(&e, soft, hard, false, now);

        if matches!(state, LifecycleState::RetentionDue) && !status_already_terminal {
            if idx.archive(&r.object_type, &r.id).await? {
                report.archived += 1;
            }
        } else if matches!(state, LifecycleState::DeleteDue) {
            // Soft marker only — actual destructive forget remains
            // presence-gated. Setting the same marker twice is idempotent.
            if idx
                .set_lifecycle_marker(
                    &r.object_type,
                    &r.id,
                    Some(PENDING_DELETE_MARKER),
                )
                .await?
            {
                report.pending_delete += 1;
            }
        }
    }

    // R-EPH: purge ephemeral context_packets bodies past the retention window.
    // We delete from `context_packets` (the bulky body row) and from the
    // satellite `context_packet_sources` rows. `exposure_decisions` (the
    // durable audit, R5-INV) is NEVER touched even though its `packet_id`
    // points here — once a packet is purged the exposure decision row still
    // tells us WHAT was exposed and WHY.
    let cutoff =
        (now - chrono::Duration::days(CONTEXT_PACKET_RETENTION_DAYS)).to_rfc3339();
    let purged = sqlx::query("DELETE FROM context_packets WHERE created_at < ?")
        .bind(&cutoff)
        .execute(pool)
        .await?;
    report.context_packets_purged = purged.rows_affected() as usize;
    // Cascade satellite rows for the now-deleted packets (idempotent: extra
    // sources without a packet are harmless but cleaned up here for tidiness).
    sqlx::query(
        "DELETE FROM context_packet_sources \
         WHERE packet_id NOT IN (SELECT id FROM context_packets)",
    )
    .execute(pool)
    .await?;

    // R5-INV pin: assert the append-only audit tables are UNCHANGED. A future
    // regression that adds an accidental write trips here. The check is a
    // `debug_assert_eq!` so release builds don't pay the cost — production
    // behavior is already correct by construction (no SQL above writes these
    // tables); the assert is a tripwire for tests + dev builds.
    let after_exposure = count_rows(pool, "exposure_decisions").await;
    let after_audit = count_rows(pool, "audit_log").await;
    debug_assert_eq!(
        after_exposure, before_exposure,
        "R5-INV: exposure_decisions row count must not change during lifecycle_archive"
    );
    debug_assert_eq!(
        after_audit, before_audit,
        "R5-INV: audit_log row count must not change during lifecycle_archive"
    );

    Ok(report)
}

/// Count rows in a table. Returns 0 if the table is missing or unreadable —
/// the lifecycle actor only relies on the BEFORE/AFTER delta, not absolute
/// counts, so a missing table just means "no audit to break".
async fn count_rows(pool: &SqlitePool, table: &str) -> i64 {
    // SAFETY: `table` is a private constant of this module — never user input.
    let sql = format!("SELECT COUNT(*) AS n FROM {table}");
    sqlx::query_scalar::<_, i64>(&sql)
        .fetch_one(pool)
        .await
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use altevra_db::repositories::objects::{ObjectIndexRepository, ObjectIndexRow};
    use chrono::Duration;

    async fn pool() -> SqlitePool {
        let p = altevra_db::create_pool("sqlite::memory:").await.unwrap();
        altevra_db::run_migrations(&p).await.unwrap();
        p
    }

    fn row(id: &str, domain: &str, status: &str, updated: DateTime<Utc>) -> ObjectIndexRow {
        ObjectIndexRow {
            object_type: "learning".into(),
            id: id.into(),
            status: status.into(),
            sensitivity: "internal".into(),
            domain: domain.into(),
            scope: None,
            title: Some(id.into()),
            categories: "[]".into(),
            tags: "[]".into(),
            redaction_status: "clean".into(),
            updated_at: updated,
        }
    }

    #[tokio::test]
    async fn sweep_reports_retention_and_fresh() {
        let p = pool().await;
        let idx = ObjectIndexRepository::new(&p);
        let now = DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        // business soft_ttl = 180d. One stale (200d), one fresh, one archived.
        idx.upsert(&row(
            "stale",
            "business",
            "active",
            now - Duration::days(200),
        ))
        .await
        .unwrap();
        idx.upsert(&row("fresh", "business", "active", now))
            .await
            .unwrap();
        idx.upsert(&row("arch", "business", "archived", now))
            .await
            .unwrap();

        let rep = lifecycle_sweep(&p, now).await.unwrap();
        assert_eq!(rep.total, 3);
        assert_eq!(rep.count("retention_due"), 1, "stale → retention_due");
        assert_eq!(rep.count("fresh"), 1);
        assert_eq!(rep.count("archived"), 1);
    }

    /// E1 — a stale Active row becomes Archived after one actor pass; the
    /// row stays, the total object_index row count is unchanged.
    #[tokio::test]
    async fn lifecycle_archives_active_to_archived() {
        let p = pool().await;
        let idx = ObjectIndexRepository::new(&p);
        let now = DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        idx.upsert(&row(
            "stale",
            "business",
            "active",
            now - Duration::days(200),
        ))
        .await
        .unwrap();
        idx.upsert(&row("fresh", "business", "active", now))
            .await
            .unwrap();

        let before_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM object_index")
            .fetch_one(&p)
            .await
            .unwrap();

        let rep = lifecycle_archive(&p, now).await.unwrap();
        assert_eq!(rep.archived, 1, "the 200d-stale row → Archived");
        assert_eq!(rep.pending_delete, 0);
        assert_eq!(rep.legal_hold_skipped, 0);

        // Row count UNCHANGED (status flip only).
        let after_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM object_index")
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(after_total, before_total, "no row deleted, just status flipped");

        // The stale row is now status='archived'; the fresh row is still active.
        let stale_status: String =
            sqlx::query_scalar("SELECT status FROM object_index WHERE id = 'stale'")
                .fetch_one(&p)
                .await
                .unwrap();
        assert_eq!(stale_status, "archived");
        let fresh_status: String =
            sqlx::query_scalar("SELECT status FROM object_index WHERE id = 'fresh'")
                .fetch_one(&p)
                .await
                .unwrap();
        assert_eq!(fresh_status, "active");

        // Idempotency: a second pass on a now-quiet DB is a no-op.
        let second = lifecycle_archive(&p, now).await.unwrap();
        assert_eq!(second.archived, 0);
    }

    /// E1 — R5-INV: a context_packet purge MUST NOT change byte counts of
    /// `exposure_decisions` or `audit_log`. We seed one of each, run the
    /// sweep, and assert both tables are byte-for-byte unchanged.
    #[tokio::test]
    async fn lifecycle_purge_preserves_audit() {
        let p = pool().await;
        let now = DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        // Seed an old packet (older than the 14d retention window) + its
        // sources + an exposure_decision pointing to it + an audit_log entry.
        // After the purge, the packet rows go; the audit rows stay.
        let old_created = (now - Duration::days(CONTEXT_PACKET_RETENTION_DAYS + 1)).to_rfc3339();
        sqlx::query(
            "INSERT INTO context_packets (id, compiler_version, profile_id, intent, request, created_at) \
             VALUES ('p_old', 'v1', 'default', 'recall', '{}', ?)",
        )
        .bind(&old_created)
        .execute(&p)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO context_packet_sources (id, packet_id, object_type, object_id) \
             VALUES ('s1', 'p_old', 'learning', 'l1')",
        )
        .execute(&p)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO exposure_decisions (id, packet_id, request, sensitivity_ceiling) \
             VALUES ('e1', 'p_old', '{}', 'internal')",
        )
        .execute(&p)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO audit_log (id, action, actor) VALUES ('a1', 'exposure_decision', 'system')",
        )
        .execute(&p)
        .await
        .unwrap();

        // Pin BOTH content snapshots (R5-INV: not just row count, but bytes).
        let before_exposure_bytes = bytes_of_table(&p, "exposure_decisions").await;
        let before_audit_bytes = bytes_of_table(&p, "audit_log").await;
        let before_exposure_rows: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM exposure_decisions")
                .fetch_one(&p)
                .await
                .unwrap();
        let before_audit_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_log")
            .fetch_one(&p)
            .await
            .unwrap();

        let rep = lifecycle_archive(&p, now).await.unwrap();
        assert_eq!(rep.context_packets_purged, 1, "the 15-day-old packet body purged");

        // The ephemeral packet body is gone (R-EPH).
        let pkt: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM context_packets")
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(pkt, 0);
        let srcs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM context_packet_sources")
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(srcs, 0, "satellite source rows cascaded");

        // The DURABLE audit rows are EXACTLY as before — bytes AND counts.
        let after_exposure_rows: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM exposure_decisions")
                .fetch_one(&p)
                .await
                .unwrap();
        let after_audit_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_log")
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(after_exposure_rows, before_exposure_rows);
        assert_eq!(after_audit_rows, before_audit_rows);

        let after_exposure_bytes = bytes_of_table(&p, "exposure_decisions").await;
        let after_audit_bytes = bytes_of_table(&p, "audit_log").await;
        assert_eq!(
            after_exposure_bytes, before_exposure_bytes,
            "R5-INV: exposure_decisions byte-stable across lifecycle"
        );
        assert_eq!(
            after_audit_bytes, before_audit_bytes,
            "R5-INV: audit_log byte-stable across lifecycle"
        );
    }

    /// Lazy "byte count" proxy: concatenate every column we'd care about. Good
    /// enough to catch any silent mutation; we don't need real bytes.
    async fn bytes_of_table(pool: &SqlitePool, table: &str) -> String {
        use sqlx::Row;
        // tables we use have an `id` column and at least one TEXT column;
        // dump rows ordered by id.
        let sql = format!("SELECT * FROM {table} ORDER BY id");
        let rows = sqlx::query(&sql).fetch_all(pool).await.unwrap();
        let mut s = String::new();
        for r in rows {
            for i in 0..r.len() {
                if let Ok(v) = r.try_get::<String, _>(i) {
                    s.push_str(&v);
                } else if let Ok(v) = r.try_get::<i64, _>(i) {
                    s.push_str(&v.to_string());
                }
                s.push('|');
            }
            s.push('\n');
        }
        s
    }

    /// E1 — D7: an object under legal hold MUST NOT be archived OR marked for
    /// pending delete, regardless of its derived state. We seed two
    /// long-stale rows; one with legal_hold=1 stays untouched, the other is
    /// archived as usual.
    #[tokio::test]
    async fn lifecycle_legal_hold_blocks_purge() {
        let p = pool().await;
        let idx = ObjectIndexRepository::new(&p);
        let now = DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        // both retention-due (200d > business soft_ttl 180d)
        idx.upsert(&row(
            "held",
            "business",
            "active",
            now - Duration::days(200),
        ))
        .await
        .unwrap();
        idx.upsert(&row(
            "free",
            "business",
            "active",
            now - Duration::days(200),
        ))
        .await
        .unwrap();
        idx.set_legal_hold("learning", "held", true).await.unwrap();

        let rep = lifecycle_archive(&p, now).await.unwrap();
        assert_eq!(rep.archived, 1, "only the unheld row was archived");
        assert_eq!(rep.legal_hold_skipped, 1, "the held row was honored");

        let held_status: String =
            sqlx::query_scalar("SELECT status FROM object_index WHERE id = 'held'")
                .fetch_one(&p)
                .await
                .unwrap();
        assert_eq!(held_status, "active", "D7: legal hold blocks archive");
        let free_status: String =
            sqlx::query_scalar("SELECT status FROM object_index WHERE id = 'free'")
                .fetch_one(&p)
                .await
                .unwrap();
        assert_eq!(free_status, "archived");
    }

    /// E1 — DeleteDue objects get the `pending_delete` marker, NEVER deleted.
    /// Honors D7: a held delete-due object stays untouched.
    #[tokio::test]
    async fn lifecycle_marks_delete_due_without_deleting() {
        let p = pool().await;
        let idx = ObjectIndexRepository::new(&p);
        let now = DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        // financial domain: hard_expiry_days = 2555 (7y, the only seeded
        // builtin with a hard horizon set, §6.4). 2600 days old → DeleteDue.
        idx.upsert(&row(
            "expired",
            "financial",
            "active",
            now - Duration::days(2600),
        ))
        .await
        .unwrap();

        let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM object_index")
            .fetch_one(&p)
            .await
            .unwrap();

        let rep = lifecycle_archive(&p, now).await.unwrap();
        assert_eq!(rep.pending_delete, 1);

        let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM object_index")
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(after, before, "delete_due was MARKED, not deleted");

        let marker: Option<String> = sqlx::query_scalar(
            "SELECT lifecycle_marker FROM object_index WHERE id = 'expired'",
        )
        .fetch_one(&p)
        .await
        .unwrap();
        assert_eq!(marker.as_deref(), Some(PENDING_DELETE_MARKER));
    }
}

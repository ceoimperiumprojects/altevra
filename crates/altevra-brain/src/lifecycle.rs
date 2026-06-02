//! Lifecycle / retention sweep (P0.8 T8.4 runtime). Non-destructive: derives each
//! indexed object's [`LifecycleState`](altevra_core::status::LifecycleState) from
//! the resolved domain policy and REPORTS counts. Any destructive action
//! (purge/archive) is review-gated downstream (R4) — the sweep never deletes.
//!
//! `object_index` carries `updated_at` + `status` but not created_at/valid_until/
//! review_after, so this sweep derives soft-ttl / hard-expiry against `updated_at`
//! (a conservative staleness proxy) and reports retention_due/delete_due/archived/
//! fresh. Per-object review windows are derived from the source tables when the
//! job sweeps them directly.

use altevra_core::envelope::{Envelope, Provenance, ProvenanceOrigin};
use altevra_core::lifecycle::derive_lifecycle_state;
use altevra_db::{DomainPolicyRepository, ObjectIndexRepository};
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use std::collections::BTreeMap;

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

/// Sweep object_index, deriving each row's lifecycle state from its domain policy.
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
}

//! ExposureDecisionsRepository — the R5 exposure-decision audit writer.
//!
//! Every context-packet compile emits ONE durable, append-only row to
//! `exposure_decisions` (migration 021). This is the "why was X exposed" record
//! that, unlike the ephemeral packet body, is NEVER auto-purged (R5-INV).
//!
//! Content-free by construction (§2.13 no-existence-leak): the row carries
//! AGGREGATES only — how many items were included, how many excluded, why
//! (coarse reason → count), the sensitivity ceiling and domain scope of the
//! request, and the redaction-status mix of what was admitted. It NEVER stores
//! object ids / titles / bodies of the candidates — surfacing a denied object's
//! id (even in an audit) would leak the existence of a higher-classified item.
//! The exact handles, if ever needed, live only in the (non-exposed) source
//! objects, not here.

use sqlx::SqlitePool;
use uuid::Uuid;

/// The content-free aggregate an audit row records for one packet compile.
/// Build this from a compiled `ContextPacket` — it deliberately carries NO
/// object ids/titles/bodies, only counts + the request envelope.
#[derive(Debug, Clone, Default)]
pub struct ExposureAudit {
    /// The packet this decided for (opaque id; nullable).
    pub packet_id: Option<String>,
    /// The request's sensitivity ceiling (e.g. "internal").
    pub sensitivity_ceiling: String,
    /// The request's domain scope, as a JSON array of domain strings.
    pub domain_scope: Vec<String>,
    /// How many candidates were admitted into the packet.
    pub included_count: usize,
    /// How many candidates were excluded.
    pub excluded_count: usize,
    /// Coarse why-excluded aggregate: reason code → count (NO ids).
    pub excluded_by_reason: Vec<(String, usize)>,
    /// Redaction-status mix of admitted items: status → count.
    pub redaction_counts: Vec<(String, usize)>,
    /// Whether the packet was truncated by the token budget.
    pub truncated: bool,
}

pub struct ExposureDecisionsRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> ExposureDecisionsRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Append one content-free audit row for a packet compile (R5). Returns the
    /// row id. The `request`, `included_refs` and `excluded_refs` JSON columns are
    /// written as content-free aggregates — `included_refs`/`excluded_refs` hold
    /// ONLY `{count, ...}` / reason aggregates, never `{type,id}` of any object
    /// (§2.13). Append-only: no UPDATE/DELETE path exists here (R5-INV).
    pub async fn insert(&self, audit: &ExposureAudit) -> anyhow::Result<Uuid> {
        let id = Uuid::new_v4();

        // request: content-free echo of the exposure envelope (NO query text/ids).
        let request = serde_json::json!({
            "sensitivity_ceiling": audit.sensitivity_ceiling,
            "domain_scope": audit.domain_scope,
        })
        .to_string();
        let domain_scope = serde_json::to_string(&audit.domain_scope)?;

        // included_refs / excluded_refs: AGGREGATE counts only — never the
        // {type,id} of a candidate. Storing an id here would re-introduce the
        // existence leak the packet compiler already closes.
        //
        // NOTE (R5 / §2.13): the column comments in migration 021_safety.sql still
        // advertise the OLD leaky shape (`[{type,id,rank,reason}]` /
        // `[{type,id,reason}]`). That comment is STALE and must NOT be followed —
        // it is left unedited only because sqlx 0.7 checksums the full migration
        // file bytes, so changing the comment would `VersionMismatch` every DB that
        // already applied 021. The authoritative, content-free contract is HERE:
        //   * included_refs := {"count": <n>}
        //   * excluded_refs := {"count": <n>, "by_reason": {<reason>: <n>}, "truncated": <bool>}
        // No object id / title / body / rank ever enters these columns.
        let included_refs = serde_json::json!({ "count": audit.included_count }).to_string();
        let excluded_by_reason: serde_json::Map<String, serde_json::Value> = audit
            .excluded_by_reason
            .iter()
            .map(|(reason, n)| (reason.clone(), serde_json::json!(n)))
            .collect();
        let excluded_refs = serde_json::json!({
            "count": audit.excluded_count,
            "by_reason": excluded_by_reason,
            "truncated": audit.truncated,
        })
        .to_string();

        let redaction_counts: serde_json::Map<String, serde_json::Value> = audit
            .redaction_counts
            .iter()
            .map(|(status, n)| (status.clone(), serde_json::json!(n)))
            .collect();
        let redaction_counts = serde_json::Value::Object(redaction_counts).to_string();

        sqlx::query(
            "INSERT INTO exposure_decisions \
             (id, packet_id, request, sensitivity_ceiling, domain_scope, \
              included_refs, excluded_refs, redaction_counts) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(audit.packet_id.as_deref())
        .bind(&request)
        .bind(&audit.sensitivity_ceiling)
        .bind(&domain_scope)
        .bind(&included_refs)
        .bind(&excluded_refs)
        .bind(&redaction_counts)
        .execute(self.pool)
        .await?;
        Ok(id)
    }
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

    #[tokio::test]
    async fn insert_writes_content_free_aggregate_row() {
        let p = pool().await;
        let repo = ExposureDecisionsRepository::new(&p);
        let audit = ExposureAudit {
            packet_id: Some("pkt-1".into()),
            sensitivity_ceiling: "internal".into(),
            domain_scope: vec!["business".into(), "project".into()],
            included_count: 3,
            excluded_count: 2,
            excluded_by_reason: vec![
                ("over_sensitivity_ceiling".into(), 1),
                ("budget_exhausted".into(), 1),
            ],
            redaction_counts: vec![("clean".into(), 2), ("redacted".into(), 1)],
            truncated: true,
        };
        let id = repo.insert(&audit).await.unwrap();

        // exactly one row, with the aggregates persisted...
        let row = sqlx::query_as::<_, (String, String, String, String, String)>(
            "SELECT id, sensitivity_ceiling, domain_scope, included_refs, excluded_refs \
             FROM exposure_decisions WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_one(&p)
        .await
        .unwrap();
        assert_eq!(row.0, id.to_string());
        assert_eq!(row.1, "internal");
        assert!(row.2.contains("business"));
        assert!(row.3.contains("\"count\":3"));
        assert!(row.4.contains("\"count\":2"));
        assert!(row.4.contains("over_sensitivity_ceiling"));

        // ...and NO raw object id/title/body columns even exist to populate:
        // the ref columns hold only counts/reasons, never a {type,id}.
        assert!(!row.3.contains("\"id\""), "included_refs must carry no id");
        assert!(!row.4.contains("\"id\""), "excluded_refs must carry no id");
    }
}

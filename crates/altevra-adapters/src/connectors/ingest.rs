//! Connector ingest path (PLAN-EXTEND §E1.3) — the ONE choke point through
//! which everything a connector pulls reaches the database.
//!
//! For every [`ConnectorItem`]:
//!   1. **guard_text** — title + body are scrubbed (secrets/PII redacted in
//!      place, fingerprint-only sightings returned) and a content-derived
//!      sensitivity is computed.
//!   2. **domain floor** — the persisted sensitivity is RAISED to the item's
//!      declared domain's policy floor (R3 most-restrictive; never lowered).
//!   3. **persist** — an `events` row (provenance + classification) AND an
//!      `object_index` + FTS entry (so the item is recallable + brief-able),
//!      keyed by `connector:external_id` (idempotent re-pull).
//!
//! NOTHING bypasses this. A connector that pulls 50 events produces 50 guarded,
//! domain-floored, provenance-stamped rows — and any embedded token is redacted
//! with a sighting, never persisted raw.

use altevra_core::events::{ActorType, Event, EventType};
use altevra_core::security::Sensitivity;
use altevra_db::{EventsRepository, ObjectIndexRepository, ObjectIndexRow};
use altevra_secrets::{guard_text, SecretSighting};
use chrono::Utc;
use sqlx::SqlitePool;

use super::{domain_sensitivity_floor, ConnectorItem};

/// What happened to one ingested item.
#[derive(Debug, Clone)]
pub struct IngestedItem {
    pub object_type: String,
    pub object_id: String,
    pub sensitivity: Sensitivity,
    pub redaction_status: String,
    /// Fingerprint-only sightings for any embedded secret (audit; never values).
    pub sightings: Vec<SecretSighting>,
}

/// Aggregate outcome of an ingest pass.
#[derive(Debug, Clone, Default)]
pub struct IngestOutcome {
    pub persisted: usize,
    pub total_sightings: usize,
    pub items: Vec<IngestedItem>,
}

impl IngestOutcome {
    pub fn summary(&self) -> String {
        format!(
            "{} item(s) persisted, {} secret sighting(s) redacted + logged",
            self.persisted, self.total_sightings
        )
    }
}

/// Persist a batch of pulled items through the full gate stack. Idempotent: the
/// object id is `connector:external_id`, so a re-pull REPLACES rather than
/// duplicates. Each item's `events` row carries the provenance + the guarded
/// classification; the `object_index`/FTS entry makes it recallable.
pub async fn ingest_items(
    pool: &SqlitePool,
    connector_name: &str,
    items: &[ConnectorItem],
) -> anyhow::Result<IngestOutcome> {
    let events = EventsRepository::new(pool);
    let idx = ObjectIndexRepository::new(pool);
    let mut outcome = IngestOutcome::default();

    for item in items {
        // ---- 1. guard the item text (secrets/PII redacted; sensitivity computed) ----
        let title_guard = guard_text(&item.payload.title(), Sensitivity::Internal);
        let body_guard = guard_text(&item.payload.guardable_text(), Sensitivity::Internal);

        let mut sightings = title_guard.sightings.clone();
        sightings.extend(body_guard.sightings.clone());

        // Content-derived sensitivity = max of the two guards.
        let content_sensitivity =
            title_guard.sensitivity.combine(&body_guard.sensitivity);

        // ---- 2. domain sensitivity FLOOR (R3, never lowers) ----
        let floor = domain_sensitivity_floor(&item.domain);
        let sensitivity = content_sensitivity.combine(&floor);

        // redaction status: redacted if either guard redacted.
        let redaction_status = if title_guard.redaction_status
            == altevra_core::status::RedactionStatus::Redacted
            || body_guard.redaction_status == altevra_core::status::RedactionStatus::Redacted
        {
            "redacted"
        } else {
            "clean"
        };

        let object_type = item.payload.object_type();
        let object_id = item.object_id();
        let guarded_title = title_guard.value.clone();
        let guarded_body = body_guard.value.clone();

        // ---- 3a. events row (provenance + classification) ----
        let payload = serde_json::json!({
            "connector": item.provenance.connector,
            "external_id": item.provenance.external_id,
            "object_type": object_type,
            "domain": item.domain.to_string(),
            "redaction_status": redaction_status,
            // The source timestamp (for calendar events this is the START time —
            // the brief's Calendar section filters today/tomorrow on it).
            "ts": item.provenance.ts.to_rfc3339(),
        });
        let event = Event::new(
            EventType::ConnectorSynced,
            guarded_title.clone(),
            format!("connector:{connector_name}"),
            ActorType::Adapter,
        )
        .with_actor(connector_name)
        .with_entity(object_type, &object_id)
        .with_summary(guarded_body.chars().take(200).collect::<String>())
        .with_payload(payload);
        // sensitivity is set after construction (Event::new defaults Internal).
        let event = Event { sensitivity: sensitivity.clone(), ..event };
        // Idempotent: the deterministic-ish id is random per Event, so dedup on
        // the object_index id; events use insert (audit trail can repeat). Use
        // insert (not insert_or_ignore) — each sync IS a fresh event.
        events.insert(&event).await?;

        // ---- 3b. object_index + FTS (recall + brief substrate) ----
        idx.index_object(
            &ObjectIndexRow {
                object_type: object_type.to_string(),
                id: object_id.clone(),
                status: "active".into(),
                sensitivity: sensitivity.to_string(),
                domain: item.domain.to_string(),
                scope: None,
                title: Some(guarded_title),
                categories: "[]".into(),
                tags: format!("[\"connector\",\"{connector_name}\"]"),
                redaction_status: redaction_status.to_string(),
                updated_at: Utc::now(),
            },
            &guarded_body,
        )
        .await?;

        outcome.total_sightings += sightings.len();
        outcome.persisted += 1;
        outcome.items.push(IngestedItem {
            object_type: object_type.to_string(),
            object_id,
            sensitivity,
            redaction_status: redaction_status.to_string(),
            sightings,
        });
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectors::{ConnectorPayload, ItemProvenance};
    use altevra_core::domain::Domain;
    use chrono::TimeZone;

    async fn pool() -> SqlitePool {
        let p = altevra_db::create_pool("sqlite::memory:").await.unwrap();
        altevra_db::run_migrations(&p).await.unwrap();
        p
    }

    fn note_item(id: &str, title: &str, body: &str, domain: Domain) -> ConnectorItem {
        ConnectorItem {
            payload: ConnectorPayload::Note {
                title: title.into(),
                body: body.into(),
            },
            domain,
            provenance: ItemProvenance {
                connector: "test".into(),
                external_id: id.into(),
                ts: Utc.with_ymd_and_hms(2026, 6, 12, 9, 0, 0).unwrap(),
            },
        }
    }

    #[tokio::test]
    async fn ingest_persists_event_and_object_index() {
        let p = pool().await;
        let items = vec![note_item("n1", "Standup", "talked GTM", Domain::Business)];
        let out = ingest_items(&p, "test", &items).await.unwrap();
        assert_eq!(out.persisted, 1);

        // object_index row exists + is FTS-searchable.
        let idx = ObjectIndexRepository::new(&p);
        let cands = idx.candidates(Some("business")).await.unwrap();
        assert!(cands.iter().any(|c| c.id == "test:n1"));
        // an events row was written.
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE event_type = 'connector_synced'")
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(n, 1);
    }

    #[tokio::test]
    async fn embedded_token_is_redacted_with_sighting() {
        let p = pool().await;
        // A note whose body embeds an API key — must never persist raw.
        let items = vec![note_item(
            "n2",
            "Config dump",
            "api key sk-FIXTUREfixtureFIXTUREfixture0000 inline",
            Domain::Business,
        )];
        let out = ingest_items(&p, "test", &items).await.unwrap();
        assert!(out.total_sightings >= 1, "embedded token must be sighted");
        assert_eq!(out.items[0].redaction_status, "redacted");

        // The FTS body must NOT contain the raw key.
        let row = sqlx::query_scalar::<_, String>(
            "SELECT body FROM object_fts WHERE object_id = 'test:n2'",
        )
        .fetch_one(&p)
        .await
        .unwrap();
        assert!(!row.contains("sk-FIXTUREfixtureFIXTUREfixture0000"), "token leaked: {row}");
        assert!(row.contains("[REDACTED]"));
    }

    #[tokio::test]
    async fn domain_floor_raises_sensitivity() {
        let p = pool().await;
        // Benign prose, but domain=health → floored to Restricted.
        let items = vec![note_item("n3", "Checkup", "routine note", Domain::Health)];
        let out = ingest_items(&p, "test", &items).await.unwrap();
        assert_eq!(out.items[0].sensitivity, Sensitivity::Restricted);

        // A business note with benign prose stays Internal (floor) — never under.
        let biz = vec![note_item("n4", "Sync", "nothing sensitive", Domain::Business)];
        let out2 = ingest_items(&p, "test", &biz).await.unwrap();
        assert_eq!(out2.items[0].sensitivity, Sensitivity::Internal);

        // comms (unknown) floors at Confidential (fail-closed).
        let comms = vec![note_item("n5", "Mail", "hi", Domain::Other("comms".into()))];
        let out3 = ingest_items(&p, "test", &comms).await.unwrap();
        assert_eq!(out3.items[0].sensitivity, Sensitivity::Confidential);
    }

    #[tokio::test]
    async fn re_ingest_is_idempotent_on_object_id() {
        let p = pool().await;
        let items = vec![note_item("dup", "T", "b", Domain::Business)];
        ingest_items(&p, "test", &items).await.unwrap();
        ingest_items(&p, "test", &items).await.unwrap();
        let idx = ObjectIndexRepository::new(&p);
        let n = idx
            .candidates(Some("business"))
            .await
            .unwrap()
            .iter()
            .filter(|c| c.id == "test:dup")
            .count();
        assert_eq!(n, 1, "object_index must dedup on connector:external_id");
    }
}

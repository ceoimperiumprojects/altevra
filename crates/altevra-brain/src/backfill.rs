//! Observer cold-start backfill (P4 §3, PLAN-ALIVE).
//!
//! Live event emission landed in P0, but the events table is empty for
//! everything recorded BEFORE that — the observer is blind to the history the
//! turns corpus already holds. `altevra observer backfill` synthesizes
//! **metadata-only** events from that corpus so the pattern detectors have a
//! cold-start signal.
//!
//! Hard invariants:
//!
//!   * **METADATA ONLY** — counts, turn/session IDs as refs, role, tool
//!     names. NEVER turn body content (033-era content may be weakly
//!     redacted). The payload carries `content_len`, not content.
//!   * **True idempotency, three legs:**
//!       1. deterministic event ids — UUIDv5 of `(source_turn_id, event_type)`
//!          under a fixed namespace,
//!       2. `INSERT OR IGNORE` ([`altevra_db::EventsRepository::insert_or_ignore`]),
//!       3. a watermark row (`observer_backfill_state`, migration 038) so a
//!          re-run only re-considers rows at/after the last sweep (the `>=`
//!          overlap on timestamp ties is absorbed by legs 1+2).
//!   * **Historical timestamps** — the synthetic event's `created_at` is the
//!     source turn's `created_at`, so backfill rows are invisible to the
//!     rolling `list_since` windows until an explicit one-shot
//!     `altevra observer scan --since @<epoch>`. A now()-stamped backfill
//!     would flood every 7-day window and drown live signal.
//!   * Tagged `source = "backfill"` so live and synthetic events never blur.

use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

/// `source` value stamped on every synthetic event.
pub const BACKFILL_SOURCE: &str = "backfill";

/// Fixed UUIDv5 namespace for backfill event ids (random-but-constant; the
/// determinism contract is `(namespace, source_turn_id, event_type)` →
/// same id forever).
const BACKFILL_NAMESPACE: Uuid = Uuid::from_bytes([
    0x6e, 0x1b, 0x6c, 0x0a, 0x6f, 0x52, 0x4d, 0x9c, 0x8e, 0x2a, 0x4f, 0x3d, 0x91, 0x7b, 0x55,
    0x21,
]);

/// Deterministic synthetic-event id: UUIDv5 of `(source_turn_id, event_type)`.
pub fn backfill_event_id(source_turn_id: &str, event_type: &str) -> Uuid {
    Uuid::new_v5(
        &BACKFILL_NAMESPACE,
        format!("{source_turn_id}:{event_type}").as_bytes(),
    )
}

/// Outcome of one backfill sweep.
#[derive(Debug, Default)]
pub struct BackfillReport {
    /// Source turns considered this run (at/after the watermark).
    pub turns_seen: usize,
    /// Synthetic events actually written.
    pub events_inserted: usize,
    /// Candidates whose deterministic id already existed (idempotent re-run).
    pub duplicates_skipped: usize,
    /// New watermark (newest source `created_at` swept), if any rows exist.
    pub watermark: Option<DateTime<Utc>>,
    /// Oldest synthetic event timestamp ever produced — feed this to
    /// `altevra observer scan --since @<epoch>` for the one-shot cold-start scan.
    pub earliest_event_at: Option<DateTime<Utc>>,
}

impl BackfillReport {
    /// The one-shot scan hint (`@<epoch>`), when anything was ever backfilled.
    pub fn scan_since_hint(&self) -> Option<String> {
        self.earliest_event_at
            .map(|t| format!("@{}", t.timestamp().saturating_sub(1)))
    }
}

/// One sweep of the turns corpus into metadata-only synthetic events.
/// Idempotent: running twice produces zero duplicate events.
pub async fn run_observer_backfill(pool: &SqlitePool) -> anyhow::Result<BackfillReport> {
    use altevra_core::events::{ActorType, Event, EventStatus, EventType};
    use altevra_core::security::Sensitivity;
    use altevra_db::EventsRepository;

    let mut report = BackfillReport::default();
    let repo = EventsRepository::new(pool);

    // Resume point. `>=` (not `>`) so timestamp ties at the watermark are
    // never skipped; the deterministic id + INSERT OR IGNORE absorb the overlap.
    let state = sqlx::query(
        "SELECT watermark, earliest_event_at FROM observer_backfill_state WHERE id = 'singleton'",
    )
    .fetch_optional(pool)
    .await?;
    let prior_watermark: Option<String> =
        state.as_ref().map(|r| r.get::<String, _>("watermark"));
    let prior_earliest: Option<String> = state
        .as_ref()
        .and_then(|r| r.get::<Option<String>, _>("earliest_event_at"));

    // Metadata-only projection of the turns corpus. `length(content)` is a
    // count; the content column itself NEVER leaves this query.
    let rows = sqlx::query(
        "SELECT t.id, t.session_id, t.turn_idx, t.role, t.tool_name, t.source_tool, \
                t.created_at, length(t.content) AS content_len \
         FROM turns t \
         WHERE (?1 IS NULL OR t.created_at >= ?1) \
         ORDER BY t.created_at ASC",
    )
    .bind(prior_watermark.as_deref())
    .fetch_all(pool)
    .await?;

    let mut newest: Option<String> = prior_watermark.clone();
    let mut earliest: Option<String> = prior_earliest;

    for row in &rows {
        report.turns_seen += 1;
        let turn_id: String = row.get("id");
        let session_id: String = row.get("session_id");
        let turn_idx: i64 = row.get("turn_idx");
        let role: String = row.get("role");
        let tool_name: Option<String> = row.get("tool_name");
        let source_tool: Option<String> = row.get("source_tool");
        let created_at_text: String = row.get("created_at");
        let content_len: Option<i64> = row.get("content_len");

        let created_at = chrono::DateTime::parse_from_rfc3339(&created_at_text)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());

        // Metadata-only event-type mapping: a turn that named a tool is a
        // tool-call observation; otherwise route by role.
        let event_type = if tool_name.is_some() || role == "tool_call" || role == "tool_result" {
            EventType::ToolCallObserved
        } else if role == "user" {
            EventType::PromptSent
        } else {
            EventType::ResponseReceived
        };
        let event_type_str = event_type.to_string();

        // Title is pure metadata — role + tool name, never body text.
        let title = match &tool_name {
            Some(t) => format!("backfill: {role} turn (tool: {t})"),
            None => format!("backfill: {role} turn"),
        };

        let event = Event {
            id: backfill_event_id(&turn_id, &event_type_str),
            event_type,
            project_id: None,
            actor_type: ActorType::System,
            actor_id: None,
            source: BACKFILL_SOURCE.to_string(),
            entity_type: Some("turn".to_string()),
            entity_id: Some(turn_id.clone()),
            title,
            summary: None,
            payload: serde_json::json!({
                "source": BACKFILL_SOURCE,
                "source_turn_id": turn_id,
                "session_id": session_id,
                "turn_idx": turn_idx,
                "role": role,
                "tool_name": tool_name,
                "source_tool": source_tool,
                "content_len": content_len.unwrap_or(0),
            }),
            sensitivity: Sensitivity::Internal,
            // HISTORICAL timestamp — invisible to rolling windows until the
            // explicit one-shot scan.
            created_at,
            processed_at: None,
            // Processed: the classifier must not re-chew synthetic history.
            status: EventStatus::Processed,
        };

        if repo.insert_or_ignore(&event).await? {
            report.events_inserted += 1;
        } else {
            report.duplicates_skipped += 1;
        }

        if newest.as_deref().map_or(true, |w| created_at_text.as_str() > w) {
            newest = Some(created_at_text.clone());
        }
        if earliest
            .as_deref()
            .map_or(true, |e| created_at_text.as_str() < e)
        {
            earliest = Some(created_at_text);
        }
    }

    // Watermark row: advance + accumulate. Even a no-op run records itself
    // (runs / last_run_at) so "did backfill ever run" is answerable.
    let watermark_text = newest
        .clone()
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string());
    sqlx::query(
        "INSERT INTO observer_backfill_state \
             (id, watermark, earliest_event_at, events_inserted, runs, last_run_at) \
         VALUES ('singleton', ?1, ?2, ?3, 1, strftime('%Y-%m-%dT%H:%M:%fZ','now')) \
         ON CONFLICT(id) DO UPDATE SET \
             watermark = excluded.watermark, \
             earliest_event_at = COALESCE(?2, observer_backfill_state.earliest_event_at), \
             events_inserted = observer_backfill_state.events_inserted + ?3, \
             runs = observer_backfill_state.runs + 1, \
             last_run_at = excluded.last_run_at",
    )
    .bind(&watermark_text)
    .bind(earliest.as_deref())
    .bind(report.events_inserted as i64)
    .execute(pool)
    .await?;

    report.watermark = newest
        .as_deref()
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .map(|d| d.with_timezone(&Utc));
    report.earliest_event_at = earliest
        .as_deref()
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .map(|d| d.with_timezone(&Utc));
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_id_is_deterministic_and_type_scoped() {
        let a = backfill_event_id("turn-1", "prompt_sent");
        let b = backfill_event_id("turn-1", "prompt_sent");
        let c = backfill_event_id("turn-1", "tool_call_observed");
        let d = backfill_event_id("turn-2", "prompt_sent");
        assert_eq!(a, b, "same (turn, type) → same id forever");
        assert_ne!(a, c, "different event_type → different id");
        assert_ne!(a, d, "different turn → different id");
    }
}

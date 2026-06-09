use altevra_core::events::{ActorType, Event, EventStatus, EventType};
use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::util::{opt_ts_from_text, opt_uuid_from_text, ts_from_text, ts_to_text, uuid_from_text};

pub struct EventsRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> EventsRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn insert(&self, event: &Event) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO events (id, event_type, project_id, actor_type, actor_id, source,
                entity_type, entity_id, title, summary, payload, sensitivity, created_at,
                processed_at, status)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(event.id.to_string())
        .bind(event.event_type.to_string())
        .bind(event.project_id.map(|u| u.to_string()))
        .bind(event.actor_type.to_string())
        .bind(event.actor_id.as_deref())
        .bind(&event.source)
        .bind(event.entity_type.as_deref())
        .bind(event.entity_id.as_deref())
        .bind(&event.title)
        .bind(event.summary.as_deref())
        .bind(event.payload.to_string())
        .bind(event.sensitivity.to_string())
        .bind(ts_to_text(&event.created_at))
        .bind(event.processed_at.as_ref().map(ts_to_text))
        .bind(event.status.to_string())
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Idempotent insert — `INSERT OR IGNORE` keyed on the event id. Returns
    /// `true` when a row was actually written, `false` when the id already
    /// existed. Used by the observer backfill (P4), whose deterministic
    /// UUIDv5 ids make re-runs produce zero duplicate rows.
    pub async fn insert_or_ignore(&self, event: &Event) -> anyhow::Result<bool> {
        let res = sqlx::query(
            r#"
            INSERT OR IGNORE INTO events (id, event_type, project_id, actor_type, actor_id,
                source, entity_type, entity_id, title, summary, payload, sensitivity,
                created_at, processed_at, status)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(event.id.to_string())
        .bind(event.event_type.to_string())
        .bind(event.project_id.map(|u| u.to_string()))
        .bind(event.actor_type.to_string())
        .bind(event.actor_id.as_deref())
        .bind(&event.source)
        .bind(event.entity_type.as_deref())
        .bind(event.entity_id.as_deref())
        .bind(&event.title)
        .bind(event.summary.as_deref())
        .bind(event.payload.to_string())
        .bind(event.sensitivity.to_string())
        .bind(ts_to_text(&event.created_at))
        .bind(event.processed_at.as_ref().map(ts_to_text))
        .bind(event.status.to_string())
        .execute(self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn list_since(
        &self,
        since: DateTime<Utc>,
        project_id: Option<Uuid>,
        limit: i64,
    ) -> anyhow::Result<Vec<Event>> {
        let since_text = ts_to_text(&since);
        let rows = if let Some(pid) = project_id {
            sqlx::query(
                r#"SELECT id, event_type, project_id, actor_type, actor_id, source,
                   entity_type, entity_id, title, summary, payload, sensitivity,
                   created_at, processed_at, status
                   FROM events WHERE created_at > ? AND project_id = ?
                   ORDER BY created_at DESC LIMIT ?"#,
            )
            .bind(since_text)
            .bind(pid.to_string())
            .bind(limit)
            .fetch_all(self.pool)
            .await?
        } else {
            sqlx::query(
                r#"SELECT id, event_type, project_id, actor_type, actor_id, source,
                   entity_type, entity_id, title, summary, payload, sensitivity,
                   created_at, processed_at, status
                   FROM events WHERE created_at > ?
                   ORDER BY created_at DESC LIMIT ?"#,
            )
            .bind(since_text)
            .bind(limit)
            .fetch_all(self.pool)
            .await?
        };

        let events = rows
            .into_iter()
            .map(|row| Event {
                id: uuid_from_text(row.get::<String, _>("id")),
                event_type: row
                    .get::<String, _>("event_type")
                    .parse()
                    .unwrap_or(EventType::ErrorLogged),
                project_id: opt_uuid_from_text(row.get::<Option<String>, _>("project_id")),
                actor_type: row
                    .get::<String, _>("actor_type")
                    .parse()
                    .unwrap_or(ActorType::System),
                actor_id: row.get("actor_id"),
                source: row.get("source"),
                entity_type: row.get("entity_type"),
                entity_id: row.get("entity_id"),
                title: row.get("title"),
                summary: row.get("summary"),
                payload: row
                    .get::<Option<String>, _>("payload")
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_else(|| serde_json::Value::Object(Default::default())),
                sensitivity: row
                    .get::<String, _>("sensitivity")
                    .parse()
                    .unwrap_or_default(),
                created_at: ts_from_text(row.get::<String, _>("created_at")),
                processed_at: opt_ts_from_text(row.get::<Option<String>, _>("processed_at")),
                status: row
                    .get::<String, _>("status")
                    .parse()
                    .unwrap_or(EventStatus::Pending),
            })
            .collect();

        Ok(events)
    }

    /// Pending (unprocessed) events of one type, OLDEST first — the drain
    /// order for event-driven consumers (P3c skill_reaction_judge). Oldest
    /// first so a long backlog never starves the earliest invocation.
    pub async fn list_pending_by_type(
        &self,
        event_type: &EventType,
        limit: i64,
    ) -> anyhow::Result<Vec<Event>> {
        let rows = sqlx::query(
            r#"SELECT id, event_type, project_id, actor_type, actor_id, source,
               entity_type, entity_id, title, summary, payload, sensitivity,
               created_at, processed_at, status
               FROM events WHERE event_type = ? AND status = 'pending'
               ORDER BY created_at ASC LIMIT ?"#,
        )
        .bind(event_type.to_string())
        .bind(limit)
        .fetch_all(self.pool)
        .await?;
        Ok(rows.into_iter().map(row_to_event).collect())
    }

    /// Stamp an event's terminal status (+ processed_at). Returns whether a
    /// row transitioned (false = already non-pending / unknown id).
    pub async fn mark_status(&self, id: Uuid, status: EventStatus) -> anyhow::Result<bool> {
        let res = sqlx::query(
            "UPDATE events SET status = ?, processed_at = ? \
             WHERE id = ? AND status = 'pending'",
        )
        .bind(status.to_string())
        .bind(ts_to_text(&Utc::now()))
        .bind(id.to_string())
        .execute(self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }
}

fn row_to_event(row: sqlx::sqlite::SqliteRow) -> Event {
    Event {
        id: uuid_from_text(row.get::<String, _>("id")),
        event_type: row
            .get::<String, _>("event_type")
            .parse()
            .unwrap_or(EventType::ErrorLogged),
        project_id: opt_uuid_from_text(row.get::<Option<String>, _>("project_id")),
        actor_type: row
            .get::<String, _>("actor_type")
            .parse()
            .unwrap_or(ActorType::System),
        actor_id: row.get("actor_id"),
        source: row.get("source"),
        entity_type: row.get("entity_type"),
        entity_id: row.get("entity_id"),
        title: row.get("title"),
        summary: row.get("summary"),
        payload: row
            .get::<Option<String>, _>("payload")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| serde_json::Value::Object(Default::default())),
        sensitivity: row
            .get::<String, _>("sensitivity")
            .parse()
            .unwrap_or_default(),
        created_at: ts_from_text(row.get::<String, _>("created_at")),
        processed_at: opt_ts_from_text(row.get::<Option<String>, _>("processed_at")),
        status: row
            .get::<String, _>("status")
            .parse()
            .unwrap_or(EventStatus::Pending),
    }
}

use altevra_core::events::{ActorType, Event, EventStatus, EventType};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

pub struct EventsRepository<'a> {
    pool: &'a PgPool,
}

impl<'a> EventsRepository<'a> {
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }

    pub async fn insert(&self, event: &Event) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO events (id, event_type, project_id, actor_type, actor_id, source,
                entity_type, entity_id, title, summary, payload, sensitivity, created_at,
                processed_at, status)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)
            "#,
        )
        .bind(event.id)
        .bind(event.event_type.to_string())
        .bind(event.project_id)
        .bind(event.actor_type.to_string())
        .bind(event.actor_id.as_deref())
        .bind(&event.source)
        .bind(event.entity_type.as_deref())
        .bind(event.entity_id.as_deref())
        .bind(&event.title)
        .bind(event.summary.as_deref())
        .bind(&event.payload)
        .bind(event.sensitivity.to_string())
        .bind(event.created_at)
        .bind(event.processed_at)
        .bind(event.status.to_string())
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_since(
        &self,
        since: DateTime<Utc>,
        project_id: Option<Uuid>,
        limit: i64,
    ) -> anyhow::Result<Vec<Event>> {
        let rows = if let Some(pid) = project_id {
            sqlx::query(
                r#"SELECT id, event_type, project_id, actor_type, actor_id, source,
                   entity_type, entity_id, title, summary, payload, sensitivity,
                   created_at, processed_at, status
                   FROM events WHERE created_at > $1 AND project_id = $2
                   ORDER BY created_at DESC LIMIT $3"#,
            )
            .bind(since)
            .bind(pid)
            .bind(limit)
            .fetch_all(self.pool)
            .await?
        } else {
            sqlx::query(
                r#"SELECT id, event_type, project_id, actor_type, actor_id, source,
                   entity_type, entity_id, title, summary, payload, sensitivity,
                   created_at, processed_at, status
                   FROM events WHERE created_at > $1
                   ORDER BY created_at DESC LIMIT $2"#,
            )
            .bind(since)
            .bind(limit)
            .fetch_all(self.pool)
            .await?
        };

        use sqlx::Row;
        let events = rows
            .into_iter()
            .map(|row| Event {
                id: row.get("id"),
                event_type: row
                    .get::<String, _>("event_type")
                    .parse()
                    .unwrap_or(EventType::ErrorLogged),
                project_id: row.get("project_id"),
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
                payload: row.get("payload"),
                sensitivity: row
                    .get::<String, _>("sensitivity")
                    .parse()
                    .unwrap_or_default(),
                created_at: row.get("created_at"),
                processed_at: row.get("processed_at"),
                status: row
                    .get::<String, _>("status")
                    .parse()
                    .unwrap_or(EventStatus::Pending),
            })
            .collect();

        Ok(events)
    }
}

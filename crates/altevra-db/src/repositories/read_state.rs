use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::util::{opt_uuid_from_text, ts_from_text, ts_to_text, uuid_from_text};

#[derive(Debug, Clone)]
pub struct UpdateReadState {
    pub id: Uuid,
    pub actor_type: String,
    pub actor_id: String,
    pub project_id: Option<Uuid>,
    pub last_seen_event_id: Option<Uuid>,
    pub last_seen_at: DateTime<Utc>,
}

pub struct ReadStateRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> ReadStateRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn mark_read(
        &self,
        actor_type: &str,
        actor_id: &str,
        project_id: Option<Uuid>,
        last_seen_event_id: Option<Uuid>,
    ) -> anyhow::Result<()> {
        // SQLite's UNIQUE constraint treats NULLs as distinct, so a vanilla
        // `ON CONFLICT (actor_type, actor_id, project_id)` upsert never fires
        // for project-less rows. We emulate upsert manually: try UPDATE first,
        // fall back to INSERT when no row was affected.
        let now = ts_to_text(&Utc::now());
        let project_text = project_id.map(|u| u.to_string());
        let last_seen_text = last_seen_event_id.map(|u| u.to_string());

        let result = if let Some(ref pid) = project_text {
            sqlx::query(
                r#"UPDATE update_read_state
                   SET last_seen_event_id = ?, last_seen_at = ?
                   WHERE actor_type = ? AND actor_id = ? AND project_id = ?"#,
            )
            .bind(&last_seen_text)
            .bind(&now)
            .bind(actor_type)
            .bind(actor_id)
            .bind(pid)
            .execute(self.pool)
            .await?
        } else {
            sqlx::query(
                r#"UPDATE update_read_state
                   SET last_seen_event_id = ?, last_seen_at = ?
                   WHERE actor_type = ? AND actor_id = ? AND project_id IS NULL"#,
            )
            .bind(&last_seen_text)
            .bind(&now)
            .bind(actor_type)
            .bind(actor_id)
            .execute(self.pool)
            .await?
        };

        if result.rows_affected() == 0 {
            sqlx::query(
                r#"INSERT INTO update_read_state
                    (id, actor_type, actor_id, project_id, last_seen_event_id, last_seen_at)
                   VALUES (?, ?, ?, ?, ?, ?)"#,
            )
            .bind(Uuid::new_v4().to_string())
            .bind(actor_type)
            .bind(actor_id)
            .bind(&project_text)
            .bind(&last_seen_text)
            .bind(&now)
            .execute(self.pool)
            .await?;
        }
        Ok(())
    }

    pub async fn get(
        &self,
        actor_type: &str,
        actor_id: &str,
        project_id: Option<Uuid>,
    ) -> anyhow::Result<Option<UpdateReadState>> {
        let row = if let Some(pid) = project_id {
            sqlx::query(
                r#"SELECT id, actor_type, actor_id, project_id, last_seen_event_id, last_seen_at
                   FROM update_read_state
                   WHERE actor_type = ? AND actor_id = ? AND project_id = ?"#,
            )
            .bind(actor_type)
            .bind(actor_id)
            .bind(pid.to_string())
            .fetch_optional(self.pool)
            .await?
        } else {
            sqlx::query(
                r#"SELECT id, actor_type, actor_id, project_id, last_seen_event_id, last_seen_at
                   FROM update_read_state
                   WHERE actor_type = ? AND actor_id = ? AND project_id IS NULL"#,
            )
            .bind(actor_type)
            .bind(actor_id)
            .fetch_optional(self.pool)
            .await?
        };

        Ok(row.map(|r| UpdateReadState {
            id: uuid_from_text(r.get::<String, _>("id")),
            actor_type: r.get("actor_type"),
            actor_id: r.get("actor_id"),
            project_id: opt_uuid_from_text(r.get::<Option<String>, _>("project_id")),
            last_seen_event_id: opt_uuid_from_text(
                r.get::<Option<String>, _>("last_seen_event_id"),
            ),
            last_seen_at: ts_from_text(r.get::<String, _>("last_seen_at")),
        }))
    }
}

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

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
    pool: &'a PgPool,
}

impl<'a> ReadStateRepository<'a> {
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }

    pub async fn mark_read(
        &self,
        actor_type: &str,
        actor_id: &str,
        project_id: Option<Uuid>,
        last_seen_event_id: Option<Uuid>,
    ) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO update_read_state
                (id, actor_type, actor_id, project_id, last_seen_event_id, last_seen_at)
            VALUES ($1, $2, $3, $4, $5, NOW())
            ON CONFLICT (actor_type, actor_id, project_id) DO UPDATE SET
                last_seen_event_id = EXCLUDED.last_seen_event_id,
                last_seen_at = EXCLUDED.last_seen_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(actor_type)
        .bind(actor_id)
        .bind(project_id)
        .bind(last_seen_event_id)
        .execute(self.pool)
        .await?;
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
                   WHERE actor_type = $1 AND actor_id = $2 AND project_id = $3"#,
            )
            .bind(actor_type)
            .bind(actor_id)
            .bind(pid)
            .fetch_optional(self.pool)
            .await?
        } else {
            sqlx::query(
                r#"SELECT id, actor_type, actor_id, project_id, last_seen_event_id, last_seen_at
                   FROM update_read_state
                   WHERE actor_type = $1 AND actor_id = $2 AND project_id IS NULL"#,
            )
            .bind(actor_type)
            .bind(actor_id)
            .fetch_optional(self.pool)
            .await?
        };

        Ok(row.map(|r| UpdateReadState {
            id: r.get("id"),
            actor_type: r.get("actor_type"),
            actor_id: r.get("actor_id"),
            project_id: r.get("project_id"),
            last_seen_event_id: r.get("last_seen_event_id"),
            last_seen_at: r.get("last_seen_at"),
        }))
    }
}

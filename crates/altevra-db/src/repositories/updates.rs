use altevra_core::updates::{Importance, UpdateFeedItem, UpdatesQuery};
use chrono::Utc;
use sqlx::{PgPool, Row};

pub struct UpdatesRepository<'a> {
    pool: &'a PgPool,
}

impl<'a> UpdatesRepository<'a> {
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }

    pub async fn insert(&self, item: &UpdateFeedItem) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO update_feed (id, event_id, project_id, update_type, importance,
                title, short_summary, agent_summary, affected_entities,
                recommended_agent_action, visible_to_agents, sensitivity, created_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            "#,
        )
        .bind(item.id)
        .bind(item.event_id)
        .bind(item.project_id)
        .bind(&item.update_type)
        .bind(item.importance.to_string())
        .bind(&item.title)
        .bind(&item.short_summary)
        .bind(item.agent_summary.as_deref())
        .bind(&item.affected_entities)
        .bind(item.recommended_agent_action.as_deref())
        .bind(item.visible_to_agents)
        .bind(item.sensitivity.to_string())
        .bind(item.created_at)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn query(&self, q: &UpdatesQuery) -> anyhow::Result<Vec<UpdateFeedItem>> {
        let since = q
            .since
            .unwrap_or_else(|| Utc::now() - chrono::Duration::hours(24));
        let limit: i64 = q.limit.unwrap_or(50);

        let rows = if let Some(pid) = q.project_id {
            sqlx::query(
                r#"SELECT id, event_id, project_id, update_type, importance, title,
                   short_summary, agent_summary, affected_entities, recommended_agent_action,
                   visible_to_agents, sensitivity, created_at
                   FROM update_feed
                   WHERE created_at > $1 AND project_id = $2 AND visible_to_agents = true
                     AND importance != 'noise'
                   ORDER BY created_at DESC LIMIT $3"#,
            )
            .bind(since)
            .bind(pid)
            .bind(limit)
            .fetch_all(self.pool)
            .await?
        } else {
            sqlx::query(
                r#"SELECT id, event_id, project_id, update_type, importance, title,
                   short_summary, agent_summary, affected_entities, recommended_agent_action,
                   visible_to_agents, sensitivity, created_at
                   FROM update_feed
                   WHERE created_at > $1 AND visible_to_agents = true
                     AND importance != 'noise'
                   ORDER BY created_at DESC LIMIT $2"#,
            )
            .bind(since)
            .bind(limit)
            .fetch_all(self.pool)
            .await?
        };

        let items = rows
            .into_iter()
            .map(|row| row_to_update_feed_item(&row))
            .collect();

        Ok(items)
    }

    pub async fn get_last_n(&self, n: i64) -> anyhow::Result<Vec<UpdateFeedItem>> {
        let rows = sqlx::query(
            r#"SELECT id, event_id, project_id, update_type, importance, title,
               short_summary, agent_summary, affected_entities, recommended_agent_action,
               visible_to_agents, sensitivity, created_at
               FROM update_feed WHERE visible_to_agents = true
               ORDER BY created_at DESC LIMIT $1"#,
        )
        .bind(n)
        .fetch_all(self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| row_to_update_feed_item(&row))
            .collect())
    }
}

fn row_to_update_feed_item(row: &sqlx::postgres::PgRow) -> UpdateFeedItem {
    UpdateFeedItem {
        id: row.get("id"),
        event_id: row.get("event_id"),
        project_id: row.get("project_id"),
        update_type: row.get("update_type"),
        importance: row
            .get::<String, _>("importance")
            .parse()
            .unwrap_or(Importance::Low),
        title: row.get("title"),
        short_summary: row.get("short_summary"),
        agent_summary: row.get("agent_summary"),
        affected_entities: row.get("affected_entities"),
        recommended_agent_action: row.get("recommended_agent_action"),
        visible_to_agents: row.get("visible_to_agents"),
        sensitivity: row
            .get::<String, _>("sensitivity")
            .parse()
            .unwrap_or_default(),
        created_at: row.get("created_at"),
    }
}

use altevra_core::updates::{Importance, UpdateFeedItem, UpdatesQuery};
use chrono::Utc;
use sqlx::{sqlite::SqliteRow, Row, SqlitePool};

use crate::util::{opt_uuid_from_text, ts_from_text, ts_to_text, uuid_from_text};

pub struct UpdatesRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> UpdatesRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn insert(&self, item: &UpdateFeedItem) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO update_feed (id, event_id, project_id, update_type, importance,
                title, short_summary, agent_summary, affected_entities,
                recommended_agent_action, visible_to_agents, sensitivity, created_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(item.id.to_string())
        .bind(item.event_id.to_string())
        .bind(item.project_id.map(|u| u.to_string()))
        .bind(&item.update_type)
        .bind(item.importance.to_string())
        .bind(&item.title)
        .bind(&item.short_summary)
        .bind(item.agent_summary.as_deref())
        .bind(item.affected_entities.to_string())
        .bind(item.recommended_agent_action.as_deref())
        .bind(if item.visible_to_agents { 1_i64 } else { 0_i64 })
        .bind(item.sensitivity.to_string())
        .bind(ts_to_text(&item.created_at))
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Idempotent insert keyed on the update id — `INSERT OR IGNORE`. The
    /// event_classifier derives a deterministic id (UUIDv5 of the event id) so
    /// re-running the bridge over the same events never duplicates rows.
    /// Returns `true` when a row was actually written.
    pub async fn insert_or_ignore(&self, item: &UpdateFeedItem) -> anyhow::Result<bool> {
        let res = sqlx::query(
            r#"
            INSERT OR IGNORE INTO update_feed (id, event_id, project_id, update_type, importance,
                title, short_summary, agent_summary, affected_entities,
                recommended_agent_action, visible_to_agents, sensitivity, created_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(item.id.to_string())
        .bind(item.event_id.to_string())
        .bind(item.project_id.map(|u| u.to_string()))
        .bind(&item.update_type)
        .bind(item.importance.to_string())
        .bind(&item.title)
        .bind(&item.short_summary)
        .bind(item.agent_summary.as_deref())
        .bind(item.affected_entities.to_string())
        .bind(item.recommended_agent_action.as_deref())
        .bind(if item.visible_to_agents { 1_i64 } else { 0_i64 })
        .bind(item.sensitivity.to_string())
        .bind(ts_to_text(&item.created_at))
        .execute(self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn query(&self, q: &UpdatesQuery) -> anyhow::Result<Vec<UpdateFeedItem>> {
        let since = q
            .since
            .unwrap_or_else(|| Utc::now() - chrono::Duration::hours(24));
        let limit: i64 = q.limit.unwrap_or(50);
        let since_text = ts_to_text(&since);

        let rows = if let Some(pid) = q.project_id {
            sqlx::query(
                r#"SELECT id, event_id, project_id, update_type, importance, title,
                   short_summary, agent_summary, affected_entities, recommended_agent_action,
                   visible_to_agents, sensitivity, created_at
                   FROM update_feed
                   WHERE created_at > ? AND project_id = ? AND visible_to_agents = 1
                     AND importance != 'noise'
                   ORDER BY created_at DESC LIMIT ?"#,
            )
            .bind(since_text)
            .bind(pid.to_string())
            .bind(limit)
            .fetch_all(self.pool)
            .await?
        } else {
            sqlx::query(
                r#"SELECT id, event_id, project_id, update_type, importance, title,
                   short_summary, agent_summary, affected_entities, recommended_agent_action,
                   visible_to_agents, sensitivity, created_at
                   FROM update_feed
                   WHERE created_at > ? AND visible_to_agents = 1
                     AND importance != 'noise'
                   ORDER BY created_at DESC LIMIT ?"#,
            )
            .bind(since_text)
            .bind(limit)
            .fetch_all(self.pool)
            .await?
        };

        let items = rows.into_iter().map(|row| row_to_item(&row)).collect();

        Ok(items)
    }

    pub async fn get_last_n(&self, n: i64) -> anyhow::Result<Vec<UpdateFeedItem>> {
        let rows = sqlx::query(
            r#"SELECT id, event_id, project_id, update_type, importance, title,
               short_summary, agent_summary, affected_entities, recommended_agent_action,
               visible_to_agents, sensitivity, created_at
               FROM update_feed WHERE visible_to_agents = 1
               ORDER BY created_at DESC LIMIT ?"#,
        )
        .bind(n)
        .fetch_all(self.pool)
        .await?;

        Ok(rows.into_iter().map(|row| row_to_item(&row)).collect())
    }
}

fn row_to_item(row: &SqliteRow) -> UpdateFeedItem {
    UpdateFeedItem {
        id: uuid_from_text(row.get::<String, _>("id")),
        event_id: uuid_from_text(row.get::<String, _>("event_id")),
        project_id: opt_uuid_from_text(row.get::<Option<String>, _>("project_id")),
        update_type: row.get("update_type"),
        importance: row
            .get::<String, _>("importance")
            .parse()
            .unwrap_or(Importance::Low),
        title: row.get("title"),
        short_summary: row.get("short_summary"),
        agent_summary: row.get("agent_summary"),
        affected_entities: row
            .get::<Option<String>, _>("affected_entities")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| serde_json::Value::Array(vec![])),
        recommended_agent_action: row.get("recommended_agent_action"),
        visible_to_agents: row.get::<i64, _>("visible_to_agents") != 0,
        sensitivity: row
            .get::<String, _>("sensitivity")
            .parse()
            .unwrap_or_default(),
        created_at: ts_from_text(row.get::<String, _>("created_at")),
    }
}

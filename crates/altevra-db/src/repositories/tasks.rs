use chrono::{DateTime, NaiveDate, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct TaskRow {
    pub id: Uuid,
    pub project_id: Option<Uuid>,
    pub title: String,
    pub description: Option<String>,
    pub status: String,
    pub priority: String,
    pub assignee: Option<String>,
    pub due_at: Option<DateTime<Utc>>,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct GoalRow {
    pub id: Uuid,
    pub project_id: Option<Uuid>,
    pub title: String,
    pub description: Option<String>,
    pub target_date: Option<NaiveDate>,
    pub status: String,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct DecisionRow {
    pub id: Uuid,
    pub project_id: Option<Uuid>,
    pub title: String,
    pub rationale: Option<String>,
    pub decided_at: DateTime<Utc>,
    pub decided_by: Option<String>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct ReviewItemRow {
    pub id: Uuid,
    pub project_id: Option<Uuid>,
    pub kind: String,
    pub title: String,
    pub body: Option<String>,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub metadata: serde_json::Value,
}

pub struct TasksRepository<'a> {
    pool: &'a PgPool,
}

impl<'a> TasksRepository<'a> {
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }

    pub async fn upsert_task(&self, t: &TaskRow) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO tasks (id, project_id, title, description, status, priority,
                assignee, due_at, metadata, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            ON CONFLICT (id) DO UPDATE SET
                title = EXCLUDED.title,
                description = EXCLUDED.description,
                status = EXCLUDED.status,
                priority = EXCLUDED.priority,
                assignee = EXCLUDED.assignee,
                due_at = EXCLUDED.due_at,
                metadata = EXCLUDED.metadata,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(t.id)
        .bind(t.project_id)
        .bind(&t.title)
        .bind(t.description.as_deref())
        .bind(&t.status)
        .bind(&t.priority)
        .bind(t.assignee.as_deref())
        .bind(t.due_at)
        .bind(&t.metadata)
        .bind(t.created_at)
        .bind(t.updated_at)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_active(
        &self,
        project_id: Option<Uuid>,
        limit: i64,
    ) -> anyhow::Result<Vec<TaskRow>> {
        let rows = if let Some(pid) = project_id {
            sqlx::query(
                r#"SELECT id, project_id, title, description, status, priority, assignee,
                   due_at, metadata, created_at, updated_at
                   FROM tasks WHERE project_id = $1 AND status NOT IN ('completed', 'cancelled')
                   ORDER BY priority DESC, due_at ASC NULLS LAST LIMIT $2"#,
            )
            .bind(pid)
            .bind(limit)
            .fetch_all(self.pool)
            .await?
        } else {
            sqlx::query(
                r#"SELECT id, project_id, title, description, status, priority, assignee,
                   due_at, metadata, created_at, updated_at
                   FROM tasks WHERE status NOT IN ('completed', 'cancelled')
                   ORDER BY priority DESC, due_at ASC NULLS LAST LIMIT $1"#,
            )
            .bind(limit)
            .fetch_all(self.pool)
            .await?
        };

        Ok(rows.into_iter().map(row_to_task).collect())
    }

    pub async fn save_decision(&self, d: &DecisionRow) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO decisions (id, project_id, title, rationale, decided_at, decided_by, metadata)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(d.id)
        .bind(d.project_id)
        .bind(&d.title)
        .bind(d.rationale.as_deref())
        .bind(d.decided_at)
        .bind(d.decided_by.as_deref())
        .bind(&d.metadata)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_goals(&self, project_id: Option<Uuid>) -> anyhow::Result<Vec<GoalRow>> {
        let rows = if let Some(pid) = project_id {
            sqlx::query(
                r#"SELECT id, project_id, title, description, target_date, status, metadata,
                   created_at, updated_at FROM goals WHERE project_id = $1 ORDER BY created_at DESC"#,
            )
            .bind(pid)
            .fetch_all(self.pool)
            .await?
        } else {
            sqlx::query(
                r#"SELECT id, project_id, title, description, target_date, status, metadata,
                   created_at, updated_at FROM goals ORDER BY created_at DESC"#,
            )
            .fetch_all(self.pool)
            .await?
        };

        Ok(rows
            .into_iter()
            .map(|r| GoalRow {
                id: r.get("id"),
                project_id: r.get("project_id"),
                title: r.get("title"),
                description: r.get("description"),
                target_date: r.get("target_date"),
                status: r.get("status"),
                metadata: r.get("metadata"),
                created_at: r.get("created_at"),
                updated_at: r.get("updated_at"),
            })
            .collect())
    }

    pub async fn create_review_item(&self, item: &ReviewItemRow) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO review_items (id, project_id, kind, title, body, status, created_at, metadata)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
        )
        .bind(item.id)
        .bind(item.project_id)
        .bind(&item.kind)
        .bind(&item.title)
        .bind(item.body.as_deref())
        .bind(&item.status)
        .bind(item.created_at)
        .bind(&item.metadata)
        .execute(self.pool)
        .await?;
        Ok(())
    }
}

fn row_to_task(r: sqlx::postgres::PgRow) -> TaskRow {
    TaskRow {
        id: r.get("id"),
        project_id: r.get("project_id"),
        title: r.get("title"),
        description: r.get("description"),
        status: r.get("status"),
        priority: r.get("priority"),
        assignee: r.get("assignee"),
        due_at: r.get("due_at"),
        metadata: r.get("metadata"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    }
}

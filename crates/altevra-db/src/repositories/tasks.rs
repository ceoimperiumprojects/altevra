use chrono::{DateTime, NaiveDate, Utc};
use sqlx::{sqlite::SqliteRow, Row, SqlitePool};
use uuid::Uuid;

use crate::repositories::objects::{ObjectIndexRepository, ObjectIndexRow};
use crate::util::{
    date_to_text, opt_date_from_text, opt_ts_from_text, opt_uuid_from_text, ts_from_text,
    ts_to_text, uuid_from_text,
};

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

/// The retrieval-envelope a caller carries so a decision can enter the index +
/// FTS substrate when it is written (T1.13). The `decisions` table itself stores
/// no domain/sensitivity/redaction columns, so — exactly like the
/// `LearningsRepository` contract (caller-guards) — the verdict travels here from
/// the upstream `guard_text`/`ingest_guard` call. The write path indexes ONLY
/// when `redaction_status` is a scanned verdict (`clean`/`redacted`); an
/// `unscanned`/`quarantined`/`rejected` decision is persisted but NOT indexed
/// (fail-closed — un-guarded text never becomes a recall/packet candidate).
#[derive(Debug, Clone)]
pub struct DecisionIndexEnvelope {
    pub status: String,
    pub sensitivity: String,
    pub domain: String,
    pub scope: Option<String>,
    pub categories: String,       // JSON array
    pub tags: String,             // JSON array
    pub redaction_status: String, // result of guard_text/ingest_guard
}

impl DecisionIndexEnvelope {
    /// True only for scanned verdicts that are safe to index (fail-closed).
    fn is_indexable(&self) -> bool {
        matches!(self.redaction_status.as_str(), "clean" | "redacted")
    }
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
    pool: &'a SqlitePool,
}

impl<'a> TasksRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn upsert_task(&self, t: &TaskRow) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO tasks (id, project_id, title, description, status, priority,
                assignee, due_at, metadata, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT (id) DO UPDATE SET
                title = excluded.title,
                description = excluded.description,
                status = excluded.status,
                priority = excluded.priority,
                assignee = excluded.assignee,
                due_at = excluded.due_at,
                metadata = excluded.metadata,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(t.id.to_string())
        .bind(t.project_id.map(|u| u.to_string()))
        .bind(&t.title)
        .bind(t.description.as_deref())
        .bind(&t.status)
        .bind(&t.priority)
        .bind(t.assignee.as_deref())
        .bind(t.due_at.as_ref().map(ts_to_text))
        .bind(t.metadata.to_string())
        .bind(ts_to_text(&t.created_at))
        .bind(ts_to_text(&t.updated_at))
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_active(
        &self,
        project_id: Option<Uuid>,
        limit: i64,
    ) -> anyhow::Result<Vec<TaskRow>> {
        // SQLite doesn't support `NULLS LAST` in standard ORDER BY but it does
        // sort NULL first by default for ASC. We emulate NULLS LAST by sorting
        // on (due_at IS NULL, due_at).
        let rows = if let Some(pid) = project_id {
            sqlx::query(
                r#"SELECT id, project_id, title, description, status, priority, assignee,
                   due_at, metadata, created_at, updated_at
                   FROM tasks WHERE project_id = ? AND status NOT IN ('completed', 'cancelled')
                   ORDER BY priority DESC, (due_at IS NULL) ASC, due_at ASC LIMIT ?"#,
            )
            .bind(pid.to_string())
            .bind(limit)
            .fetch_all(self.pool)
            .await?
        } else {
            sqlx::query(
                r#"SELECT id, project_id, title, description, status, priority, assignee,
                   due_at, metadata, created_at, updated_at
                   FROM tasks WHERE status NOT IN ('completed', 'cancelled')
                   ORDER BY priority DESC, (due_at IS NULL) ASC, due_at ASC LIMIT ?"#,
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
            VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(d.id.to_string())
        .bind(d.project_id.map(|u| u.to_string()))
        .bind(&d.title)
        .bind(d.rationale.as_deref())
        .bind(ts_to_text(&d.decided_at))
        .bind(d.decided_by.as_deref())
        .bind(d.metadata.to_string())
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Save a decision AND route it into the retrieval substrate (T1.13): the
    /// decision becomes a packet candidate (`object_index`) + full-text searchable
    /// (`object_fts`) immediately, the same single-maintenance-point contract the
    /// `LearningsRepository` already honors. The indexed body is `title` + the
    /// rationale (the searchable prose of a decision).
    ///
    /// Fail-closed: if `idx.redaction_status` is not a scanned verdict
    /// (`clean`/`redacted`), the decision is still persisted but is NOT indexed —
    /// un-guarded text must never enter the index (R11 / TAG-1). The caller is
    /// responsible for having run `guard_text`/`ingest_guard` upstream and passing
    /// the resulting verdict (caller-guards, no double-guard).
    pub async fn save_decision_indexed(
        &self,
        d: &DecisionRow,
        idx: &DecisionIndexEnvelope,
    ) -> anyhow::Result<()> {
        self.save_decision(d).await?;
        if !idx.is_indexable() {
            return Ok(());
        }
        let body = match &d.rationale {
            Some(r) => format!("{}\n\n{}", d.title, r),
            None => d.title.clone(),
        };
        ObjectIndexRepository::new(self.pool)
            .index_object(
                &ObjectIndexRow {
                    object_type: "decision".into(),
                    id: d.id.to_string(),
                    status: idx.status.clone(),
                    sensitivity: idx.sensitivity.clone(),
                    domain: idx.domain.clone(),
                    scope: idx.scope.clone(),
                    title: Some(d.title.clone()),
                    categories: idx.categories.clone(),
                    tags: idx.tags.clone(),
                    redaction_status: idx.redaction_status.clone(),
                    updated_at: d.decided_at,
                },
                &body,
            )
            .await?;
        Ok(())
    }

    pub async fn list_goals(&self, project_id: Option<Uuid>) -> anyhow::Result<Vec<GoalRow>> {
        let rows = if let Some(pid) = project_id {
            sqlx::query(
                r#"SELECT id, project_id, title, description, target_date, status, metadata,
                   created_at, updated_at FROM goals WHERE project_id = ? ORDER BY created_at DESC"#,
            )
            .bind(pid.to_string())
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

        Ok(rows.into_iter().map(row_to_goal).collect())
    }

    /// Upsert a goal (used by tests + downstream sync logic). Kept additive
    /// to the previous public API.
    pub async fn upsert_goal(&self, g: &GoalRow) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO goals (id, project_id, title, description, target_date, status, metadata,
                created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT (id) DO UPDATE SET
                title = excluded.title,
                description = excluded.description,
                target_date = excluded.target_date,
                status = excluded.status,
                metadata = excluded.metadata,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(g.id.to_string())
        .bind(g.project_id.map(|u| u.to_string()))
        .bind(&g.title)
        .bind(g.description.as_deref())
        .bind(g.target_date.as_ref().map(date_to_text))
        .bind(&g.status)
        .bind(g.metadata.to_string())
        .bind(ts_to_text(&g.created_at))
        .bind(ts_to_text(&g.updated_at))
        .execute(self.pool)
        .await?;
        Ok(())
    }

    pub async fn create_review_item(&self, item: &ReviewItemRow) -> anyhow::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO review_items (id, project_id, kind, title, body, status, created_at, metadata)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(item.id.to_string())
        .bind(item.project_id.map(|u| u.to_string()))
        .bind(&item.kind)
        .bind(&item.title)
        .bind(item.body.as_deref())
        .bind(&item.status)
        .bind(ts_to_text(&item.created_at))
        .bind(item.metadata.to_string())
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// List review items, optionally filtered by status, newest first (P0.3 T3.1).
    pub async fn list_review_items(
        &self,
        status: Option<&str>,
        limit: i64,
    ) -> anyhow::Result<Vec<ReviewItemRow>> {
        let base = "SELECT id, project_id, kind, title, body, status, created_at, metadata FROM review_items";
        let rows = if let Some(s) = status {
            sqlx::query(&format!(
                "{base} WHERE status = ? ORDER BY created_at DESC LIMIT ?"
            ))
            .bind(s)
            .bind(limit)
            .fetch_all(self.pool)
            .await?
        } else {
            sqlx::query(&format!("{base} ORDER BY created_at DESC LIMIT ?"))
                .bind(limit)
                .fetch_all(self.pool)
                .await?
        };
        Ok(rows.into_iter().map(row_to_review).collect())
    }

    pub async fn get_review_item(&self, id: Uuid) -> anyhow::Result<Option<ReviewItemRow>> {
        let row = sqlx::query(
            "SELECT id, project_id, kind, title, body, status, created_at, metadata \
             FROM review_items WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(self.pool)
        .await?;
        Ok(row.map(row_to_review))
    }

    /// Record an approve/reject decision. The caller MUST have verified human
    /// presence FIRST (HP-2: approval is recorded by core after a presence check,
    /// never accepted as an input flag). Returns true if a row transitioned.
    pub async fn decide_review_item(
        &self,
        id: Uuid,
        decision: &str,
        decided_by: &str,
    ) -> anyhow::Result<bool> {
        let status = match decision {
            "approved" => "approved",
            "rejected" => "rejected",
            other => anyhow::bail!("invalid decision: {other} (expected approved|rejected)"),
        };
        let now = ts_to_text(&Utc::now());
        let res = sqlx::query(
            "UPDATE review_items SET status = ?, decision = ?, decided_by = ?, decided_at = ?, \
             updated_at = ? WHERE id = ? AND status IN ('open','pending_review')",
        )
        .bind(status)
        .bind(decision)
        .bind(decided_by)
        .bind(&now)
        .bind(&now)
        .bind(id.to_string())
        .execute(self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }
}

fn row_to_review(r: SqliteRow) -> ReviewItemRow {
    ReviewItemRow {
        id: uuid_from_text(r.get::<String, _>("id")),
        project_id: opt_uuid_from_text(r.get::<Option<String>, _>("project_id")),
        kind: r.get("kind"),
        title: r.get("title"),
        body: r.get("body"),
        status: r.get("status"),
        created_at: ts_from_text(r.get::<String, _>("created_at")),
        metadata: r
            .get::<Option<String>, _>("metadata")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| serde_json::json!({})),
    }
}

fn row_to_task(r: SqliteRow) -> TaskRow {
    TaskRow {
        id: uuid_from_text(r.get::<String, _>("id")),
        project_id: opt_uuid_from_text(r.get::<Option<String>, _>("project_id")),
        title: r.get("title"),
        description: r.get("description"),
        status: r.get("status"),
        priority: r.get("priority"),
        assignee: r.get("assignee"),
        due_at: opt_ts_from_text(r.get::<Option<String>, _>("due_at")),
        metadata: r
            .get::<Option<String>, _>("metadata")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| serde_json::Value::Object(Default::default())),
        created_at: ts_from_text(r.get::<String, _>("created_at")),
        updated_at: ts_from_text(r.get::<String, _>("updated_at")),
    }
}

fn row_to_goal(r: SqliteRow) -> GoalRow {
    GoalRow {
        id: uuid_from_text(r.get::<String, _>("id")),
        project_id: opt_uuid_from_text(r.get::<Option<String>, _>("project_id")),
        title: r.get("title"),
        description: r.get("description"),
        target_date: opt_date_from_text(r.get::<Option<String>, _>("target_date")),
        status: r.get("status"),
        metadata: r
            .get::<Option<String>, _>("metadata")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| serde_json::Value::Object(Default::default())),
        created_at: ts_from_text(r.get::<String, _>("created_at")),
        updated_at: ts_from_text(r.get::<String, _>("updated_at")),
    }
}

use serde_json::{json, Value};
use std::path::PathBuf;
use uuid::Uuid;

use crate::server::McpResponse;

// DB-backed task/goal/decision tools. These previously read/wrote a JSON file
// under ~/.altevra/state/ that nothing else used, so the SQLite `tasks`/`goals`/
// `decisions` tables (which the brain + daily brief read) stayed empty. Now they
// hit the real altevra_db repositories.

fn db_path_from_args(args: &Value) -> PathBuf {
    args.get("db_path")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .unwrap_or_else(altevra_core::default_db_path)
}

/// Run an async DB closure on a fresh single-thread runtime (same pattern as
/// tools_capabilities). Keeps these handlers synchronous for the dispatcher.
fn with_pool<T, F>(db_path: PathBuf, f: F) -> anyhow::Result<T>
where
    T: Send + 'static,
    F: FnOnce(
            sqlx::SqlitePool,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = anyhow::Result<T>> + Send>,
        > + Send
        + 'static,
{
    std::thread::spawn(move || -> anyhow::Result<T> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        rt.block_on(async move {
            let pool = altevra_db::create_pool(&db_path.to_string_lossy()).await?;
            altevra_db::run_migrations(&pool).await?;
            f(pool).await
        })
    })
    .join()
    .map_err(|_| anyhow::anyhow!("task DB thread panicked"))?
}

pub fn handle_get_active_tasks(id: Value, args: &Value) -> McpResponse {
    let db_path = db_path_from_args(args);
    let res = with_pool(db_path, |pool| {
        Box::pin(async move {
            let tasks = altevra_db::TasksRepository::new(&pool)
                .list_active(None, 100)
                .await?;
            let arr: Vec<Value> = tasks
                .iter()
                .map(|t| {
                    json!({
                        "id": t.id.to_string(),
                        "title": t.title,
                        "status": t.status,
                        "priority": t.priority,
                        "due_at": t.due_at,
                        "created_at": t.created_at,
                    })
                })
                .collect();
            Ok(arr)
        })
    });
    match res {
        Ok(arr) => McpResponse::ok(id, json!({"tasks": arr, "count": arr.len()})),
        Err(e) => McpResponse::error(id, -32000, format!("get_active_tasks failed: {e}")),
    }
}

pub fn handle_save_task(id: Value, args: &Value) -> McpResponse {
    let title = match args["title"].as_str() {
        Some(t) => t.to_string(),
        None => return McpResponse::error(id, -32602, "missing 'title'"),
    };
    let status = args["status"].as_str().unwrap_or("open").to_string();
    let priority = args["priority"].as_str().unwrap_or("medium").to_string();
    let db_path = db_path_from_args(args);
    let now = chrono::Utc::now();
    let task_id = Uuid::new_v4();
    let res = with_pool(db_path, move |pool| {
        Box::pin(async move {
            let row = altevra_db::TaskRow {
                id: task_id,
                project_id: None,
                title,
                description: None,
                status,
                priority,
                assignee: None,
                due_at: None,
                metadata: json!({}),
                created_at: now,
                updated_at: now,
            };
            altevra_db::TasksRepository::new(&pool).upsert_task(&row).await?;
            Ok(())
        })
    });
    match res {
        Ok(()) => McpResponse::ok(id, json!({"id": task_id.to_string(), "saved": true})),
        Err(e) => McpResponse::error(id, -32000, format!("save_task failed: {e}")),
    }
}

pub fn handle_update_task(id: Value, args: &Value) -> McpResponse {
    let task_id = match args["id"].as_str().and_then(|s| Uuid::parse_str(s).ok()) {
        Some(t) => t,
        None => return McpResponse::error(id, -32602, "missing or invalid 'id'"),
    };
    let new_status = args["status"].as_str().map(str::to_string);
    let new_priority = args["priority"].as_str().map(str::to_string);
    let db_path = db_path_from_args(args);
    let res = with_pool(db_path, move |pool| {
        Box::pin(async move {
            let repo = altevra_db::TasksRepository::new(&pool);
            // No get-by-id; find it in the active set, mutate, re-upsert.
            let mut all = repo.list_active(None, 1000).await?;
            let Some(row) = all.iter_mut().find(|t| t.id == task_id) else {
                anyhow::bail!("task not found: {task_id}");
            };
            if let Some(s) = new_status {
                row.status = s;
            }
            if let Some(p) = new_priority {
                row.priority = p;
            }
            row.updated_at = chrono::Utc::now();
            repo.upsert_task(row).await?;
            Ok(())
        })
    });
    match res {
        Ok(()) => McpResponse::ok(id, json!({"updated": true, "id": task_id.to_string()})),
        Err(e) => McpResponse::error(id, -32000, format!("update_task failed: {e}")),
    }
}

pub fn handle_get_goals(id: Value, args: &Value) -> McpResponse {
    let db_path = db_path_from_args(args);
    let res = with_pool(db_path, |pool| {
        Box::pin(async move {
            let goals = altevra_db::TasksRepository::new(&pool).list_goals(None).await?;
            let arr: Vec<Value> = goals
                .iter()
                .map(|g| {
                    json!({
                        "id": g.id.to_string(),
                        "title": g.title,
                        "status": g.status,
                        "target_date": g.target_date,
                        "created_at": g.created_at,
                    })
                })
                .collect();
            Ok(arr)
        })
    });
    match res {
        Ok(arr) => McpResponse::ok(id, json!({"goals": arr, "count": arr.len()})),
        Err(e) => McpResponse::error(id, -32000, format!("get_goals failed: {e}")),
    }
}

pub fn handle_save_decision(id: Value, args: &Value) -> McpResponse {
    let title = match args["title"].as_str() {
        Some(t) => t.to_string(),
        None => return McpResponse::error(id, -32602, "missing 'title'"),
    };
    let rationale = args["rationale"].as_str().map(str::to_string);
    let decided_by = args["decided_by"].as_str().map(str::to_string);
    let db_path = db_path_from_args(args);
    let dec_id = Uuid::new_v4();
    let res = with_pool(db_path, move |pool| {
        Box::pin(async move {
            let row = altevra_db::DecisionRow {
                id: dec_id,
                project_id: None,
                title,
                rationale,
                decided_at: chrono::Utc::now(),
                decided_by,
                metadata: json!({}),
            };
            altevra_db::TasksRepository::new(&pool).save_decision(&row).await?;
            Ok(())
        })
    });
    match res {
        Ok(()) => McpResponse::ok(id, json!({"id": dec_id.to_string(), "saved": true})),
        Err(e) => McpResponse::error(id, -32000, format!("save_decision failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_task_missing_title_errors() {
        let resp = handle_save_task(Value::from(1), &serde_json::json!({}));
        assert!(resp.error.is_some());
    }
}

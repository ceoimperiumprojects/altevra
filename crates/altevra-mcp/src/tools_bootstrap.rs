use altevra_bootstrap::{BootstrapBuilder, SetupStatus};
use altevra_skills::registry::SkillRegistry;
use serde_json::Value;
use std::path::Path;

use crate::server::McpResponse;

pub fn handle_bootstrap(
    id: Value,
    args: &Value,
    altevra_version: &str,
    vault_path: &Path,
) -> McpResponse {
    let tool_name = args["tool_name"].as_str().unwrap_or("unknown");
    let project = args["project"].as_str();
    let installed_skill_version = args["installed_skill_version"].as_str();

    // Load skills from vault so freshness check reflects real state.
    let mut registry = SkillRegistry::new();
    let skills_dir = vault_path.join("06-skills");
    if skills_dir.is_dir() {
        let _ = load_skills_from_dir(&mut registry, &skills_dir);
    }

    let freshness = vec![altevra_bootstrap::freshness::FreshnessCheck::check(
        &registry,
        "altevra-core",
        installed_skill_version,
    )];

    // §P2.1 Hermes transport leg: the bootstrap packet carries the SAME gated
    // session-context block + Tool Register summaries the Claude Code hook
    // injects. `db_path` defaults to the canonical DB (same convention as the
    // other DB-backed handlers); fault-tolerant: a DB error degrades to an
    // empty register / no block — bootstrap never fails over a locked DB.
    let db = args["db_path"]
        .as_str()
        .map(std::string::ToString::to_string)
        .unwrap_or_else(|| altevra_core::default_db_path().to_string_lossy().into_owned());
    // Same runtime guard the other DB-backed handlers use: sqlx needs a tokio
    // context; without one, degrade to an empty register (never an error).
    let (available_tools, session_context) = if tokio::runtime::Handle::try_current().is_ok() {
        futures::executor::block_on(async move {
            let run = async {
                let pool = altevra_db::create_pool(&db).await?;
                altevra_db::run_migrations(&pool).await?;
                anyhow::Ok(
                    altevra_bootstrap::session_context::bootstrap_context(
                        &pool,
                        &format!("bootstrap_packet:{}", uuid::Uuid::new_v4()),
                    )
                    .await,
                )
            };
            run.await.unwrap_or((vec![], None))
        })
    } else {
        (vec![], None)
    };

    let mut builder = BootstrapBuilder::new(tool_name, altevra_version)
        .skill_freshness(freshness)
        .setup_status(SetupStatus::placeholder(tool_name))
        .available_tools(available_tools)
        .session_context(session_context);

    if let Some(p) = project {
        builder = builder.project(p);
    }

    let packet = builder.build();

    match serde_json::to_value(&packet) {
        Ok(val) => McpResponse::ok(id, val),
        Err(e) => McpResponse::error(id, -32603, format!("Serialization error: {e}")),
    }
}

fn load_skills_from_dir(registry: &mut SkillRegistry, dir: &Path) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir)?.flatten() {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let content = std::fs::read_to_string(&p)?;
        let _ = registry.register(p.to_string_lossy().as_ref(), &content);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hermetic per-test DB path (handler default would be the REAL canonical
    /// DB — tests must always name a TempDir db).
    fn tmp_db(tmp: &tempfile::TempDir) -> String {
        tmp.path().join("bootstrap-test.db").to_string_lossy().into_owned()
    }

    #[test]
    fn test_bootstrap_tool_returns_packet() {
        let tmp = tempfile::TempDir::new().unwrap();
        let id = serde_json::json!(1);
        let args = serde_json::json!({
            "tool_name": "claude-code",
            "project": "altevra",
            "db_path": tmp_db(&tmp),
        });
        let resp = handle_bootstrap(id, &args, "0.1.0", tmp.path());
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["tool_name"], "claude-code");
        assert_eq!(result["project"], "altevra");
        // §P2 #7: the packet carries the (possibly empty) tool register.
        assert!(result["available_tools"].is_array());
    }

    #[test]
    fn test_bootstrap_no_project() {
        let tmp = tempfile::TempDir::new().unwrap();
        let id = serde_json::json!(2);
        let args = serde_json::json!({"tool_name": "codex", "db_path": tmp_db(&tmp)});
        let resp = handle_bootstrap(id, &args, "0.1.0", tmp.path());
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["tool_name"], "codex");
        assert!(result["project"].is_null());
    }

    #[test]
    fn test_bootstrap_loads_skill_from_vault() {
        let tmp = tempfile::TempDir::new().unwrap();
        let skills_dir = tmp.path().join("06-skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        std::fs::write(
            skills_dir.join("altevra-core.md"),
            "---\nslug: altevra-core\nversion: 0.5.0\ntitle: Altevra Core\n---\nBody.",
        )
        .unwrap();

        let id = serde_json::json!(3);
        let args = serde_json::json!({
            "tool_name": "claude-code",
            "installed_skill_version": "0.5.0",
            "db_path": tmp_db(&tmp),
        });
        let resp = handle_bootstrap(id, &args, "0.1.0", tmp.path());
        let result = resp.result.unwrap();
        let freshness = &result["skill_freshness"][0];
        assert_eq!(freshness["status"], "current");
        assert!(freshness["action_required"].is_null());
    }

    #[test]
    fn test_bootstrap_skill_outdated_via_mcp() {
        let tmp = tempfile::TempDir::new().unwrap();
        let skills_dir = tmp.path().join("06-skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        std::fs::write(
            skills_dir.join("altevra-core.md"),
            "---\nslug: altevra-core\nversion: 0.5.0\ntitle: Altevra Core\n---\nBody.",
        )
        .unwrap();

        let id = serde_json::json!(4);
        let args = serde_json::json!({
            "tool_name": "claude-code",
            "installed_skill_version": "0.4.0",
            "db_path": tmp_db(&tmp),
        });
        let resp = handle_bootstrap(id, &args, "0.1.0", tmp.path());
        let result = resp.result.unwrap();
        assert_eq!(result["skill_freshness"][0]["status"], "outdated");
    }

    /// §P2.1 Hermes leg: the bootstrap packet gains the SAME content the hook
    /// injects — tool register + gated session-context block.
    #[tokio::test(flavor = "multi_thread")]
    async fn test_bootstrap_carries_tool_register_and_session_context() {
        use altevra_db::{ObjectIndexRepository, ObjectIndexRow, ToolRecordRow, ToolRecordsRepository};

        let tmp = tempfile::TempDir::new().unwrap();
        let db = tmp_db(&tmp);
        let pool = altevra_db::create_pool(&db).await.unwrap();
        altevra_db::run_migrations(&pool).await.unwrap();
        let mut t = ToolRecordRow::new("imperium-crawl", "cli");
        t.invocation = serde_json::json!({"canonical": "imperium-crawl <cmd>"});
        t.source = "manual".into();
        ToolRecordsRepository::new(&pool).upsert(&t).await.unwrap();
        ObjectIndexRepository::new(&pool)
            .upsert(&ObjectIndexRow {
                object_type: "decision".into(),
                id: "d1".into(),
                status: "active".into(),
                sensitivity: "internal".into(),
                domain: "business".into(),
                scope: None,
                title: Some("ONE canonical DB".into()),
                categories: "[\"business\"]".into(),
                tags: "[]".into(),
                redaction_status: "clean".into(),
                updated_at: chrono::Utc::now(),
            })
            .await
            .unwrap();
        pool.close().await;

        let db2 = db.clone();
        let vault = tmp.path().to_path_buf();
        // handle_bootstrap uses block_on internally — run it off the runtime.
        let resp = tokio::task::spawn_blocking(move || {
            handle_bootstrap(
                serde_json::json!(5),
                &serde_json::json!({"tool_name": "hermes", "db_path": db2}),
                "0.1.0",
                &vault,
            )
        })
        .await
        .unwrap();
        assert!(resp.error.is_none(), "{:?}", resp.error);
        let result = resp.result.unwrap();
        let tools = result["available_tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "imperium-crawl");
        assert_eq!(tools[0]["invocation"], "imperium-crawl <cmd>");
        let block = result["session_context"].as_str().unwrap();
        assert!(block.contains("ONE canonical DB"));
        assert!(block.contains("=== ALTEVRA TOOL REGISTER ==="));
    }
}

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

    let mut builder = BootstrapBuilder::new(tool_name, altevra_version)
        .skill_freshness(freshness)
        .setup_status(SetupStatus::placeholder(tool_name));

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
    use std::path::PathBuf;

    #[test]
    fn test_bootstrap_tool_returns_packet() {
        let id = serde_json::json!(1);
        let args = serde_json::json!({
            "tool_name": "claude-code",
            "project": "altevra"
        });
        let resp = handle_bootstrap(id, &args, "0.1.0", &PathBuf::from("."));
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["tool_name"], "claude-code");
        assert_eq!(result["project"], "altevra");
    }

    #[test]
    fn test_bootstrap_no_project() {
        let id = serde_json::json!(2);
        let args = serde_json::json!({"tool_name": "codex"});
        let resp = handle_bootstrap(id, &args, "0.1.0", &PathBuf::from("."));
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
            "installed_skill_version": "0.5.0"
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
            "installed_skill_version": "0.4.0"
        });
        let resp = handle_bootstrap(id, &args, "0.1.0", tmp.path());
        let result = resp.result.unwrap();
        assert_eq!(result["skill_freshness"][0]["status"], "outdated");
    }
}

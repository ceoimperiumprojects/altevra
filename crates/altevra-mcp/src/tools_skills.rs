use altevra_skills::registry::SkillRegistry;
use serde_json::Value;

use crate::server::McpResponse;

pub fn handle_check_skill_version(id: Value, args: &Value) -> McpResponse {
    let skill_slug = args["skill_slug"].as_str().unwrap_or("altevra-core");
    let installed_version = args["installed_version"].as_str();

    // In MVP: return status relative to known defaults.
    // Real implementation loads registry from vault/DB.
    let registry = SkillRegistry::new();
    let check = altevra_bootstrap::freshness::FreshnessCheck::check(
        &registry,
        skill_slug,
        installed_version,
    );

    match serde_json::to_value(&check) {
        Ok(val) => McpResponse::ok(id, val),
        Err(e) => McpResponse::error(id, -32603, format!("Serialization error: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_skill_not_installed() {
        let id = serde_json::json!(1);
        let args = serde_json::json!({
            "skill_slug": "altevra-core"
        });
        let resp = handle_check_skill_version(id, &args);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["skill_slug"], "altevra-core");
        assert_eq!(result["status"], "not_installed");
    }

    #[test]
    fn test_check_skill_with_version() {
        let id = serde_json::json!(2);
        let args = serde_json::json!({
            "skill_slug": "altevra-core",
            "installed_version": "0.5.0"
        });
        let resp = handle_check_skill_version(id, &args);
        assert!(resp.error.is_none());
    }
}

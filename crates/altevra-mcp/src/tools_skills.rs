use altevra_skills::{parser::parse_skill, registry::SkillRegistry};
use serde_json::Value;
use std::path::Path;

use crate::server::McpResponse;

pub fn handle_check_skill_version(id: Value, args: &Value) -> McpResponse {
    let skill_slug = args["skill_slug"].as_str().unwrap_or("altevra-core");
    let installed_version = args["installed_version"].as_str();

    let registry = load_registry(args["vault"].as_str().unwrap_or("."));
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

pub fn handle_get_altevra_skill(id: Value, args: &Value) -> McpResponse {
    let vault = args["vault"].as_str().unwrap_or(".");
    let slug = args["slug"].as_str().unwrap_or("altevra-core");
    let path = Path::new(vault)
        .join("06-skills")
        .join(format!("{slug}.md"));
    if !path.exists() {
        return McpResponse::error(id, -32000, format!("skill not found: {slug}"));
    }
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    match parse_skill(&content) {
        Ok(p) => McpResponse::ok(
            id,
            serde_json::json!({
                "slug": p.frontmatter.slug,
                "version": p.frontmatter.version,
                "title": p.frontmatter.title,
                "description": p.frontmatter.description,
                "body": p.body,
                "source": path,
            }),
        ),
        Err(e) => McpResponse::error(id, -32000, format!("parse error: {e}")),
    }
}

pub fn handle_get_skill(id: Value, args: &Value) -> McpResponse {
    handle_get_altevra_skill(id, args)
}

pub fn handle_list_skills(id: Value, args: &Value) -> McpResponse {
    let vault = args["vault"].as_str().unwrap_or(".");
    let registry = load_registry(vault);
    let entries: Vec<_> = registry
        .list()
        .iter()
        .map(|e| {
            serde_json::json!({
                "slug": e.slug(),
                "version": e.skill.frontmatter.version,
                "title": e.skill.frontmatter.title,
                "checksum": e.checksum,
            })
        })
        .collect();
    McpResponse::ok(
        id,
        serde_json::json!({"skills": entries.clone(), "count": entries.len()}),
    )
}

pub fn handle_request_skill_refresh(id: Value, args: &Value) -> McpResponse {
    let slug = match args["slug"].as_str() {
        Some(s) => s.to_string(),
        None => return McpResponse::error(id, -32602, "missing 'slug'"),
    };
    // Refresh is performed by the CLI in practice; here we ack the request.
    McpResponse::ok(
        id,
        serde_json::json!({
            "requested": true,
            "slug": slug,
            "next_step": format!("Run: altevra skill refresh {slug}"),
        }),
    )
}

fn load_registry(vault: &str) -> SkillRegistry {
    let mut registry = SkillRegistry::new();
    let dir = Path::new(vault).join("06-skills");
    if !dir.exists() {
        return registry;
    }
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "md").unwrap_or(false) {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    let _ = registry.register(path.to_string_lossy().into_owned(), &content);
                }
            }
        }
    }
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_skill_not_installed() {
        let id = serde_json::json!(1);
        let args = serde_json::json!({"skill_slug": "altevra-core"});
        let resp = handle_check_skill_version(id, &args);
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["skill_slug"], "altevra-core");
        assert_eq!(result["status"], "not_installed");
    }

    #[test]
    fn test_check_skill_with_version() {
        let id = serde_json::json!(2);
        let args = serde_json::json!({"skill_slug": "altevra-core", "installed_version": "0.5.0"});
        let resp = handle_check_skill_version(id, &args);
        assert!(resp.error.is_none());
    }

    #[test]
    fn list_skills_empty_vault() {
        let tmp = tempfile::TempDir::new().unwrap();
        let resp = handle_list_skills(
            Value::from(1),
            &serde_json::json!({"vault": tmp.path().to_string_lossy()}),
        );
        assert!(resp.error.is_none());
        assert_eq!(resp.result.unwrap()["count"], 0);
    }

    #[test]
    fn get_altevra_skill_missing_errors() {
        let tmp = tempfile::TempDir::new().unwrap();
        let resp = handle_get_altevra_skill(
            Value::from(1),
            &serde_json::json!({"vault": tmp.path().to_string_lossy(), "slug": "missing"}),
        );
        assert!(resp.error.is_some());
    }

    #[test]
    fn request_skill_refresh_missing_slug_errors() {
        let resp = handle_request_skill_refresh(Value::from(1), &serde_json::json!({}));
        assert!(resp.error.is_some());
    }
}

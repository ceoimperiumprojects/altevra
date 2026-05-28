use altevra_core::prompts::{build_for_tool, PromptInput, PromptSkill, DEFAULT_UPDATES_LIMIT};
use altevra_core::updates::{Importance, UpdateFeedItem};
use altevra_skills::registry::SkillRegistry;
use serde_json::Value;
use std::path::Path;

use crate::server::McpResponse;

pub fn handle_build_system_prompt(id: Value, args: &Value, altevra_version: &str) -> McpResponse {
    let tool_name = match args["tool_name"].as_str() {
        Some(s) if !s.trim().is_empty() => s.to_string(),
        _ => {
            return McpResponse::error(id, -32602, "Missing required arg: tool_name");
        }
    };
    let vault_str = args["vault"].as_str().unwrap_or(".");
    let vault = Path::new(vault_str);

    let limit = args["limit"]
        .as_u64()
        .map(|v| v as usize)
        .unwrap_or(DEFAULT_UPDATES_LIMIT);

    let project = args["project"].as_str().map(str::to_string);
    let current_task = args["current_task"].as_str().map(str::to_string);
    let current_goal = args["current_goal"].as_str().map(str::to_string);

    let skills = load_skills_for_prompt(vault);
    let recent_updates = load_recent_updates(vault, limit);
    let project_readme = project
        .as_deref()
        .and_then(|p| load_project_readme(vault, p));

    let input = PromptInput {
        tool_name,
        project,
        current_task,
        current_goal,
        recent_updates,
        skills,
        project_readme,
        altevra_version: altevra_version.to_string(),
    };

    let output = build_for_tool(input);
    match serde_json::to_value(&output) {
        Ok(v) => McpResponse::ok(id, v),
        Err(e) => McpResponse::error(id, -32603, format!("Serialization error: {e}")),
    }
}

fn load_skills_for_prompt(vault: &Path) -> Vec<PromptSkill> {
    let mut registry = SkillRegistry::new();
    let skills_dir = vault.join("06-skills");
    if !skills_dir.exists() {
        return vec![];
    }
    let Ok(entries) = std::fs::read_dir(&skills_dir) else {
        return vec![];
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e == "md").unwrap_or(false) {
            if let Ok(content) = std::fs::read_to_string(&path) {
                let _ = registry.register(path.display().to_string(), &content);
            }
        }
    }

    let mut out = Vec::new();
    for entry in registry.list() {
        let f = &entry.skill.frontmatter;
        let mut sk = PromptSkill::new(&f.slug, &f.version, &f.title);
        sk.description = f.description.clone();
        out.push(sk);
    }
    out
}

fn load_recent_updates(vault: &Path, limit: usize) -> Vec<UpdateFeedItem> {
    // Default: .altevra/events/updates.jsonl relative to cwd, but allow vault override
    let candidates = [
        vault.join(".altevra/events/updates.jsonl"),
        Path::new(".altevra/events/updates.jsonl").to_path_buf(),
    ];
    let mut content: Option<String> = None;
    for p in &candidates {
        if p.exists() {
            content = std::fs::read_to_string(p).ok();
            break;
        }
    }
    let Some(content) = content else {
        return vec![];
    };
    let mut items: Vec<UpdateFeedItem> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    items.sort_by_key(|i| std::cmp::Reverse(i.created_at));
    if items.len() > limit {
        items.retain(|i| i.importance >= Importance::Medium);
    }
    items.truncate(limit);
    items
}

fn load_project_readme(vault: &Path, project: &str) -> Option<String> {
    let candidates = [
        vault.join("01-projects").join(project).join("README.md"),
        vault.join("01-projects").join(project).join("readme.md"),
    ];
    for p in candidates {
        if p.exists() {
            return std::fs::read_to_string(&p).ok();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn build_system_prompt_with_all_args() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join("06-skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        std::fs::write(
            skills_dir.join("altevra-core.md"),
            "---\nslug: altevra-core\nversion: 0.6.0\ntitle: Altevra Core\n---\nBody.",
        )
        .unwrap();
        let proj_dir = tmp.path().join("01-projects").join("altevra");
        std::fs::create_dir_all(&proj_dir).unwrap();
        std::fs::write(proj_dir.join("README.md"), "# Altevra\nLocal-first.").unwrap();

        let id = serde_json::json!(1);
        let args = serde_json::json!({
            "tool_name": "claude-code",
            "project": "altevra",
            "current_task": "Ship v0.2",
            "current_goal": "Hit 100 daily users",
            "vault": tmp.path().to_str().unwrap(),
            "limit": 5,
        });

        let resp = handle_build_system_prompt(id, &args, "0.1.0");
        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        let prompt = result["system_prompt"].as_str().unwrap();
        assert!(prompt.contains("claude-code"));
        assert!(prompt.contains("altevra-core"));
        assert!(prompt.contains("Ship v0.2"));
        assert!(prompt.contains("Local-first"));
        assert!(result["layer_count"].as_u64().unwrap() >= 4);
        assert!(result["token_estimate"].as_u64().unwrap() > 0);
        assert!(result["layers_included"].is_array());
    }

    #[test]
    fn build_system_prompt_missing_tool_name_errors() {
        let id = serde_json::json!(1);
        let args = serde_json::json!({"project": "altevra"});
        let resp = handle_build_system_prompt(id, &args, "0.1.0");
        assert!(resp.error.is_some());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("tool_name"));
    }

    #[test]
    fn build_system_prompt_empty_tool_name_errors() {
        let id = serde_json::json!(1);
        let args = serde_json::json!({"tool_name": "   "});
        let resp = handle_build_system_prompt(id, &args, "0.1.0");
        assert!(resp.error.is_some());
    }
}

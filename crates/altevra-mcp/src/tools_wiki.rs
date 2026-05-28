//! MCP tools for v0.3 Phase 1: Resident Agent + Wiki Layer foundation.
//!
//! All four tools are read-only: they surface what's on disk (wiki/ + 06-skills/)
//! without mutating anything. Mutations land in Phase 5 (Wiki Curator).

use crate::server::McpResponse;
use serde_json::Value;
use std::path::{Path, PathBuf};

const DEFAULT_WIKI_ROOT: &str = "wiki";
const DEFAULT_SKILLS_ROOT: &str = "06-skills";

fn root_or(args: &Value, default: &str) -> PathBuf {
    args.get("root")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(default))
}

pub fn handle_get_wiki_page(id: Value, args: &Value) -> McpResponse {
    let topic = match args.get("topic").and_then(|v| v.as_str()) {
        Some(t) if !t.is_empty() => t,
        _ => return McpResponse::error(id, -32602, "topic required"),
    };
    let root = root_or(args, DEFAULT_WIKI_ROOT);
    match altevra_vault::list_wiki_pages(&root) {
        Ok(pages) => match pages.into_iter().find(|p| p.topic == topic) {
            Some(p) => McpResponse::ok(
                id,
                serde_json::json!({
                    "topic": p.topic,
                    "id": p.id,
                    "status": p.status.as_str(),
                    "confidence": p.confidence.as_str(),
                    "sensitivity": p.sensitivity,
                    "source_count": p.source_count,
                    "last_synthesized_at": p.last_synthesized_at,
                    "related_projects": p.related_projects,
                    "related_pages": p.related_pages,
                    "wiki_links": p.wiki_links,
                    "owner": p.owner,
                    "title": p.title,
                    "path": p.path.display().to_string(),
                    "body": p.body,
                    "checksum": p.checksum,
                }),
            ),
            None => McpResponse::error(id, -32602, format!("wiki page '{topic}' not found")),
        },
        Err(e) => McpResponse::error(id, -32603, format!("list_wiki_pages: {e}")),
    }
}

pub fn handle_search_wiki(id: Value, args: &Value) -> McpResponse {
    let query = match args.get("query").and_then(|v| v.as_str()) {
        Some(q) if !q.is_empty() => q.to_lowercase(),
        _ => return McpResponse::error(id, -32602, "query required"),
    };
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
    let root = root_or(args, DEFAULT_WIKI_ROOT);
    match altevra_vault::list_wiki_pages(&root) {
        Ok(pages) => {
            let hits: Vec<_> = pages
                .into_iter()
                .filter(|p| {
                    p.topic.to_lowercase().contains(&query)
                        || p.title
                            .as_deref()
                            .map(|t| t.to_lowercase().contains(&query))
                            .unwrap_or(false)
                        || p.body.to_lowercase().contains(&query)
                })
                .take(limit)
                .map(|p| {
                    serde_json::json!({
                        "topic": p.topic,
                        "status": p.status.as_str(),
                        "title": p.title,
                        "sensitivity": p.sensitivity,
                        "path": p.path.display().to_string(),
                    })
                })
                .collect();
            McpResponse::ok(
                id,
                serde_json::json!({
                    "query": query,
                    "count": hits.len(),
                    "results": hits,
                }),
            )
        }
        Err(e) => McpResponse::error(id, -32603, format!("list_wiki_pages: {e}")),
    }
}

pub fn handle_list_resident_modes(id: Value, args: &Value) -> McpResponse {
    let root = root_or(args, DEFAULT_SKILLS_ROOT);
    let modes = enumerate_modes(&root);
    McpResponse::ok(
        id,
        serde_json::json!({
            "count": modes.len(),
            "modes": modes,
        }),
    )
}

pub fn handle_get_resident_prompt(id: Value, args: &Value) -> McpResponse {
    let mode = match args.get("mode").and_then(|v| v.as_str()) {
        Some(m) if !m.is_empty() => m,
        _ => return McpResponse::error(id, -32602, "mode required"),
    };
    let root = root_or(args, DEFAULT_SKILLS_ROOT);
    let path = match resolve_mode_path(&root, mode) {
        Some(p) => p,
        None => return McpResponse::error(id, -32602, format!("unknown mode: '{mode}'")),
    };
    match std::fs::read_to_string(&path) {
        Ok(content) => McpResponse::ok(
            id,
            serde_json::json!({
                "mode": mode,
                "path": path.display().to_string(),
                "prompt": content,
            }),
        ),
        Err(e) => McpResponse::error(id, -32603, format!("read prompt: {e}")),
    }
}

fn enumerate_modes(root: &Path) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    let core = root.join("resident-agent-core.md");
    if core.exists() {
        out.push(serde_json::json!({
            "name": "core",
            "role": "core",
            "file": core.display().to_string(),
        }));
    }
    let modes_dir = root.join("resident-agent-modes");
    if modes_dir.exists() {
        for entry in walkdir::WalkDir::new(&modes_dir)
            .max_depth(1)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let p = entry.path();
            if p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("md") {
                let name = p
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("?")
                    .replace('-', "_");
                out.push(serde_json::json!({
                    "name": name,
                    "role": "mode",
                    "file": p.display().to_string(),
                }));
            }
        }
    }
    out
}

fn resolve_mode_path(root: &Path, mode: &str) -> Option<PathBuf> {
    if mode == "core" {
        let p = root.join("resident-agent-core.md");
        return p.exists().then_some(p);
    }
    let kebab = mode.replace('_', "-");
    let candidates = [
        root.join("resident-agent-modes")
            .join(format!("{kebab}.md")),
        root.join("resident-agent-modes").join(format!("{mode}.md")),
    ];
    candidates.into_iter().find(|p| p.exists())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn seed_wiki() -> TempDir {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("concepts")).unwrap();
        std::fs::write(
            tmp.path().join("concepts/foo.md"),
            "---\ntopic: foo\nstatus: living\n---\n# Foo\n\nfoo body\n",
        )
        .unwrap();
        tmp
    }

    fn seed_skills() -> TempDir {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("resident-agent-modes")).unwrap();
        std::fs::write(
            tmp.path().join("resident-agent-core.md"),
            "---\nid: core\n---\n# Core\n",
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("resident-agent-modes/synthesis.md"),
            "---\nmode: synthesis\n---\n# Mode: Synthesis\n",
        )
        .unwrap();
        tmp
    }

    #[test]
    fn get_wiki_page_missing_topic_errors() {
        let resp = handle_get_wiki_page(serde_json::json!(1), &serde_json::json!({}));
        assert!(resp.error.is_some());
    }

    #[test]
    fn get_wiki_page_returns_seeded_page() {
        let tmp = seed_wiki();
        let resp = handle_get_wiki_page(
            serde_json::json!(1),
            &serde_json::json!({"topic": "foo", "root": tmp.path().display().to_string()}),
        );
        assert!(resp.result.is_some());
        let v = resp.result.unwrap();
        assert_eq!(v.get("topic").and_then(|x| x.as_str()), Some("foo"));
    }

    #[test]
    fn search_wiki_missing_query_errors() {
        let resp = handle_search_wiki(serde_json::json!(1), &serde_json::json!({}));
        assert!(resp.error.is_some());
    }

    #[test]
    fn search_wiki_finds_match() {
        let tmp = seed_wiki();
        let resp = handle_search_wiki(
            serde_json::json!(1),
            &serde_json::json!({"query": "foo", "root": tmp.path().display().to_string()}),
        );
        assert!(resp.result.is_some());
        let v = resp.result.unwrap();
        assert_eq!(v.get("count").and_then(|c| c.as_u64()), Some(1));
    }

    #[test]
    fn list_resident_modes_returns_core_and_modes() {
        let tmp = seed_skills();
        let resp = handle_list_resident_modes(
            serde_json::json!(1),
            &serde_json::json!({"root": tmp.path().display().to_string()}),
        );
        let v = resp.result.unwrap();
        assert_eq!(v.get("count").and_then(|c| c.as_u64()), Some(2));
    }

    #[test]
    fn get_resident_prompt_unknown_errors() {
        let resp = handle_get_resident_prompt(
            serde_json::json!(1),
            &serde_json::json!({"mode": "noexist"}),
        );
        assert!(resp.error.is_some());
    }
}

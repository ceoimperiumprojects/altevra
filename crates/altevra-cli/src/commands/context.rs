//! `altevra context` — assembles a Retrieval Context for an agent.
//!
//! Modes:
//!   * `show` (default, legacy) — file listing summary by section
//!   * `build --query "X"` — RAG retrieval: pulls memory chunks, decisions,
//!     learnings, active tasks, recent updates, applicable skills into a
//!     `RetrievalContext` ready for an agent prompt.

use altevra_core::retrieval::{
    RetrievalChunk, RetrievalContext, RetrievalDecision, RetrievalLearning, RetrievalSkill,
    RetrievalTask,
};
use altevra_core::updates::UpdateFeedItem;
use altevra_memory::{ingest_file, SearchIndex};
use altevra_skills::{parser::parse_skill, registry::SkillRegistry};
use altevra_vault::scan_vault;
use clap::Args;
use std::path::PathBuf;

#[derive(Args)]
pub struct ContextArgs {
    /// Query to retrieve relevant context for. If omitted, runs the legacy
    /// summary mode (file count + section breakdown).
    #[arg(long)]
    pub query: Option<String>,

    /// Project filter
    #[arg(long)]
    pub project: Option<String>,

    /// Vault root
    #[arg(long, default_value = ".")]
    pub vault: PathBuf,

    /// Max memory chunks
    #[arg(long, default_value_t = 8)]
    pub chunk_limit: usize,

    /// Max recent updates
    #[arg(long, default_value_t = 10)]
    pub update_limit: usize,

    /// Output mode: markdown (default), json, prompt
    #[arg(long, default_value = "markdown")]
    pub format: String,
}

pub async fn run(args: ContextArgs) -> anyhow::Result<()> {
    if let Some(query) = args.query.clone() {
        run_build(query, args).await
    } else {
        run_show(args).await
    }
}

async fn run_build(query: String, args: ContextArgs) -> anyhow::Result<()> {
    let mut ctx = RetrievalContext::new(&query);
    ctx.project = args.project.clone();

    // 1. Memory search across the vault
    let files = scan_vault(&args.vault).unwrap_or_default();
    let mut index = SearchIndex::new();
    for f in &files {
        if let Ok(doc) = ingest_file(&f.path, 2000) {
            index.add_document(doc);
        }
    }
    let hits = index.search(&query, args.chunk_limit);
    ctx.chunks = hits
        .into_iter()
        .map(|h| RetrievalChunk {
            source_path: h.source_path.map(|p| p.to_string_lossy().into_owned()),
            heading_path: h.heading_path,
            snippet: h.snippet,
            score: h.score,
        })
        .collect();

    // 2. Decisions from 08-decisions/
    ctx.decisions = load_section_entries(&args.vault, "08-decisions", &query, 5)
        .into_iter()
        .map(|(title, body, path)| RetrievalDecision {
            title,
            rationale: Some(body),
            decided_at: None,
            source_path: Some(path),
        })
        .collect();

    // 3. Learnings from 10-insights/
    ctx.learnings = load_section_entries(&args.vault, "10-insights", &query, 5)
        .into_iter()
        .map(|(title, body, path)| RetrievalLearning {
            title,
            body,
            source_path: Some(path),
        })
        .collect();

    // 4. Active tasks (local JSON state)
    ctx.tasks = load_local_tasks(args.project.as_deref(), 5);

    // 5. Recent updates
    ctx.updates = load_recent_updates(args.update_limit);

    // 6. Applicable skills
    ctx.skills = load_applicable_skills(&args.vault);

    ctx.recompute_token_estimate();

    match args.format.as_str() {
        "json" => {
            println!("{}", serde_json::to_string_pretty(&ctx)?);
        }
        "prompt" => {
            // Use the layered prompt builder with the retrieval brief inlined.
            use altevra_core::prompts::{build_for_tool, PromptInput, PromptSkill};
            let mut input = PromptInput::new("claude-code", env!("CARGO_PKG_VERSION"));
            input.project = args.project.clone();
            input.current_task = Some(query.clone());
            input.recent_updates = ctx.updates.clone();
            input.skills = ctx
                .skills
                .iter()
                .map(|s| PromptSkill::new(&s.slug, &s.version, &s.title))
                .collect();
            // Inject the retrieval brief as project_readme so it lands in the
            // project layer of the prompt.
            input.project_readme = Some(ctx.to_markdown());
            let prompt = build_for_tool(input);
            println!("{}", prompt.system_prompt);
        }
        _ => {
            println!("{}", ctx.to_markdown());
        }
    }
    Ok(())
}

async fn run_show(args: ContextArgs) -> anyhow::Result<()> {
    let files = scan_vault(&args.vault)?;
    let project = args.project.clone();
    let project_label = project.as_deref().unwrap_or("(all)");
    let relevant: Vec<_> = files
        .into_iter()
        .filter(|f| {
            project
                .as_deref()
                .map(|p| f.path.to_string_lossy().contains(p))
                .unwrap_or(true)
        })
        .collect();
    let sections: std::collections::BTreeMap<String, usize> =
        relevant.iter().fold(Default::default(), |mut acc, f| {
            let s = f.section.clone().unwrap_or_else(|| "root".into());
            *acc.entry(s).or_insert(0) += 1;
            acc
        });

    if args.format == "json" {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "project": project_label,
                "vault_root": args.vault,
                "files": relevant.len(),
                "sections": sections,
                "recent_files": relevant.iter().take(10).map(|f| serde_json::json!({
                    "path": f.path,
                    "section": f.section,
                })).collect::<Vec<_>>(),
            }))?
        );
    } else {
        println!("Context for: {project_label}");
        println!("Vault: {}", args.vault.display());
        println!("Total files: {}", relevant.len());
        println!("\nSections:");
        for (section, count) in &sections {
            println!("  {section}: {count}");
        }
        println!("\nHint: `altevra context --query \"<topic>\"` for retrieval-augmented context.");
    }
    Ok(())
}

fn load_section_entries(
    vault: &std::path::Path,
    section: &str,
    query: &str,
    limit: usize,
) -> Vec<(String, String, String)> {
    let dir = vault.join(section);
    if !dir.exists() {
        return vec![];
    }
    let query_lower = query.to_lowercase();
    let mut entries = Vec::new();
    let walker = std::fs::read_dir(&dir).ok();
    if let Some(iter) = walker {
        for entry in iter.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            let title = content
                .lines()
                .find(|l| l.starts_with("# "))
                .map(|l| l.trim_start_matches('#').trim().to_string())
                .unwrap_or_else(|| {
                    path.file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default()
                });
            // simple relevance: query terms appearing in content
            let lc = content.to_lowercase();
            let score = query_lower
                .split_whitespace()
                .filter(|t| lc.contains(t))
                .count();
            entries.push((
                score,
                title,
                content.chars().take(400).collect::<String>(),
                path.display().to_string(),
            ));
        }
    }
    entries.sort_by_key(|e| std::cmp::Reverse(e.0));
    entries
        .into_iter()
        .take(limit)
        .map(|(_, t, b, p)| (t, b, p))
        .collect()
}

fn load_local_tasks(project_filter: Option<&str>, limit: usize) -> Vec<RetrievalTask> {
    let path = std::path::Path::new(".altevra/state/tasks.json");
    if !path.exists() {
        return vec![];
    }
    let raw = std::fs::read_to_string(path).unwrap_or_default();
    let arr: serde_json::Value =
        serde_json::from_str(&raw).unwrap_or(serde_json::Value::Array(vec![]));
    let active: Vec<RetrievalTask> = arr
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|t| {
                    t["status"]
                        .as_str()
                        .map(|s| s != "completed" && s != "cancelled")
                        .unwrap_or(true)
                })
                .filter(|t| {
                    project_filter
                        .map(|p| t["project"].as_str().map(|tp| tp == p).unwrap_or(false))
                        .unwrap_or(true)
                })
                .take(limit)
                .map(|t| RetrievalTask {
                    title: t["title"].as_str().unwrap_or("").to_string(),
                    status: t["status"].as_str().unwrap_or("open").to_string(),
                    priority: t["priority"].as_str().map(String::from),
                    due_at: t["due_at"].as_str().map(String::from),
                })
                .collect()
        })
        .unwrap_or_default();
    active
}

fn load_recent_updates(limit: usize) -> Vec<UpdateFeedItem> {
    let path = std::path::Path::new(".altevra/events/updates.jsonl");
    if !path.exists() {
        return vec![];
    }
    let content = std::fs::read_to_string(path).unwrap_or_default();
    let mut items: Vec<UpdateFeedItem> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    items.sort_by_key(|i| std::cmp::Reverse(i.created_at));
    items.truncate(limit);
    items
}

fn load_applicable_skills(vault: &std::path::Path) -> Vec<RetrievalSkill> {
    let dir = vault.join("06-skills");
    if !dir.exists() {
        return vec![];
    }
    let mut registry = SkillRegistry::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "md").unwrap_or(false) {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if parse_skill(&content).is_ok() {
                        let _ = registry.register(path.to_string_lossy().into_owned(), &content);
                    }
                }
            }
        }
    }
    registry
        .list()
        .iter()
        .map(|e| RetrievalSkill {
            slug: e.slug().to_string(),
            version: e.skill.frontmatter.version.clone(),
            title: e.skill.frontmatter.title.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn show_mode_runs_empty_vault() {
        let tmp = TempDir::new().unwrap();
        run(ContextArgs {
            query: None,
            project: None,
            vault: tmp.path().to_path_buf(),
            chunk_limit: 5,
            update_limit: 10,
            format: "json".into(),
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn build_mode_runs_with_query() {
        let tmp = TempDir::new().unwrap();
        let skills = tmp.path().join("06-skills");
        std::fs::create_dir_all(&skills).unwrap();
        std::fs::write(
            skills.join("test.md"),
            "---\nslug: test\nversion: 0.1.0\ntitle: Test Skill\n---\nBody about altevra agents.",
        )
        .unwrap();
        run(ContextArgs {
            query: Some("altevra".into()),
            project: None,
            vault: tmp.path().to_path_buf(),
            chunk_limit: 5,
            update_limit: 10,
            format: "markdown".into(),
        })
        .await
        .unwrap();
    }
}

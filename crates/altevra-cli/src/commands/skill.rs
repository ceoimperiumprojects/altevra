use altevra_skills::{
    importer::{group_by_slug, scan_all, scan_external_dir, ExternalSkill, SourceTool},
    parser::parse_skill,
    registry::{SkillRegistry, VersionCheckResult},
};
use clap::{Args, Subcommand};
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub enum SkillCommands {
    /// List all registered skills
    List(SkillListArgs),
    /// Show details of a specific skill
    Show(SkillShowArgs),
    /// Check if installed skills are current
    Check(SkillCheckArgs),
    /// Refresh a skill from source
    Refresh(SkillRefreshArgs),
    /// Inventory skills across ALL connected tools (~/.claude, ~/.codex,
    /// ~/.cursor, ~/.hermes, ~/.imperium, and the Altevra vault). Read-only.
    Inventory(SkillInventoryArgs),
}

#[derive(Args)]
pub struct SkillInventoryArgs {
    /// Filter to skills present in a specific source tool.
    #[arg(long)]
    pub tool: Option<String>,
    /// Show ONLY skills not in every tool (diff view — sync candidates).
    #[arg(long)]
    pub missing: bool,
    /// Include Altevra vault `06-skills` in the scan (relative to --vault).
    #[arg(long, default_value = ".")]
    pub vault: PathBuf,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct SkillListArgs {
    #[arg(long)]
    pub json: bool,
    #[arg(long, default_value = ".")]
    pub vault: PathBuf,
}

#[derive(Args)]
pub struct SkillShowArgs {
    pub slug: String,
    #[arg(long, default_value = ".")]
    pub vault: PathBuf,
}

#[derive(Args)]
pub struct SkillCheckArgs {
    /// Check all skills
    #[arg(long)]
    pub all: bool,
    /// Specific skill slug
    pub slug: Option<String>,
    /// Currently installed version (for single check)
    #[arg(long)]
    pub installed_version: Option<String>,
    #[arg(long, default_value = ".")]
    pub vault: PathBuf,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct SkillRefreshArgs {
    pub slug: String,
    #[arg(long, default_value = ".")]
    pub vault: PathBuf,
}

pub async fn run(cmd: SkillCommands) -> anyhow::Result<()> {
    match cmd {
        SkillCommands::List(args) => run_list(args).await,
        SkillCommands::Show(args) => run_show(args).await,
        SkillCommands::Check(args) => run_check(args).await,
        SkillCommands::Refresh(args) => run_refresh(args).await,
        SkillCommands::Inventory(args) => run_inventory(args).await,
    }
}

async fn run_inventory(args: SkillInventoryArgs) -> anyhow::Result<()> {
    // Scan every known tool's skill dir + (optionally) the Altevra vault.
    let mut skills: Vec<ExternalSkill> = scan_all();
    let vault_skills_dir = args.vault.join("06-skills");
    if vault_skills_dir.exists() {
        skills.extend(scan_external_dir(&vault_skills_dir, SourceTool::Altevra));
    }

    // Optional --tool filter.
    if let Some(t) = args.tool.as_deref() {
        skills.retain(|s| s.source_tool.as_str() == t);
    }

    let grouped = group_by_slug(&skills);

    // --missing: show only skills NOT present in every distinct tool we found.
    let tools_in_scan: std::collections::BTreeSet<&str> =
        skills.iter().map(|s| s.source_tool.as_str()).collect();
    let total_tools = tools_in_scan.len();
    let mut rows: Vec<_> = grouped.iter().collect();
    if args.missing {
        rows.retain(|(_, list)| {
            let distinct_tools: std::collections::HashSet<_> =
                list.iter().map(|s| s.source_tool.as_str()).collect();
            distinct_tools.len() < total_tools
        });
    }
    rows.sort_by(|a, b| a.0.cmp(b.0));

    if args.json {
        let out: Vec<_> = rows
            .iter()
            .map(|(slug, list)| {
                serde_json::json!({
                    "slug": slug,
                    "tools": list.iter().map(|s| s.source_tool.as_str()).collect::<Vec<_>>(),
                    "managed": list.iter().any(|s| s.managed),
                    "instances": list.iter().map(|s| serde_json::json!({
                        "tool": s.source_tool.as_str(),
                        "path": s.path,
                        "version": s.version,
                        "description": s.description,
                        "managed": s.managed,
                        "body_len": s.body_len,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!(
            "Skill inventory — {} unique slug(s) across {} tool(s){}:\n",
            rows.len(),
            total_tools,
            if args.missing {
                " (missing-in-some only)"
            } else {
                ""
            }
        );
        for (slug, list) in &rows {
            let tools: Vec<&str> = {
                let mut v: Vec<_> = list.iter().map(|s| s.source_tool.as_str()).collect();
                v.sort_unstable();
                v.dedup();
                v
            };
            let has_managed = list.iter().any(|s| s.managed);
            println!(
                "  {slug:32}  [{tools}]{m}",
                tools = tools.join(","),
                m = if has_managed { "  (managed)" } else { "" }
            );
        }
        println!("\nNext: `altevra skill inventory --missing` to see propagation candidates.");
    }
    Ok(())
}

async fn run_list(args: SkillListArgs) -> anyhow::Result<()> {
    let mut registry = SkillRegistry::new();
    let skills_dir = args.vault.join("06-skills");

    if skills_dir.exists() {
        load_skills_from_dir(&mut registry, &skills_dir)?;
    }

    let entries = registry.list();

    if args.json {
        let items: Vec<_> = entries
            .iter()
            .map(|e| {
                serde_json::json!({
                    "slug": e.slug(),
                    "version": e.skill.frontmatter.version,
                    "title": e.skill.frontmatter.title,
                    "source_path": e.source_path,
                    "checksum": e.checksum,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "skills": items,
                "count": items.len()
            }))?
        );
    } else {
        if entries.is_empty() {
            println!("No skills found in: {}", skills_dir.display());
            println!("Hint: Add skill files to 06-skills/");
        } else {
            println!("Skills ({}):", entries.len());
            for e in entries {
                let v = e
                    .version()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "?".to_string());
                println!("  {} v{} — {}", e.slug(), v, e.skill.frontmatter.title);
            }
        }
    }

    Ok(())
}

async fn run_show(args: SkillShowArgs) -> anyhow::Result<()> {
    let mut registry = SkillRegistry::new();
    let skills_dir = args.vault.join("06-skills");
    if skills_dir.exists() {
        load_skills_from_dir(&mut registry, &skills_dir)?;
    }

    match registry.get(&args.slug) {
        Some(entry) => {
            println!("Slug:     {}", entry.slug());
            println!("Title:    {}", entry.skill.frontmatter.title);
            println!("Version:  {}", entry.skill.frontmatter.version);
            println!("Checksum: {}", entry.checksum);
            println!("Source:   {}", entry.source_path);
            if let Some(desc) = &entry.skill.frontmatter.description {
                println!("Description: {desc}");
            }
            println!("\n--- Body ---\n{}", entry.skill.body);
        }
        None => {
            anyhow::bail!("Skill not found: {}", args.slug);
        }
    }
    Ok(())
}

async fn run_check(args: SkillCheckArgs) -> anyhow::Result<()> {
    let mut registry = SkillRegistry::new();
    let skills_dir = args.vault.join("06-skills");
    if skills_dir.exists() {
        load_skills_from_dir(&mut registry, &skills_dir)?;
    }

    let slugs_to_check: Vec<String> = if args.all {
        registry
            .list()
            .iter()
            .map(|e| e.slug().to_string())
            .collect()
    } else if let Some(slug) = args.slug.clone() {
        vec![slug]
    } else {
        vec!["altevra-core".to_string()]
    };

    let mut results = vec![];
    for slug in &slugs_to_check {
        let installed = args.installed_version.as_deref();
        let check_result = registry.check_version_opt(slug, installed);
        results.push(serde_json::json!({
            "slug": slug,
            "status": match &check_result {
                VersionCheckResult::Current => "current",
                VersionCheckResult::Outdated { .. } => "outdated",
                VersionCheckResult::Ahead { .. } => "ahead",
                VersionCheckResult::NotInstalled => "not_installed",
                VersionCheckResult::NotFound => "not_found",
                VersionCheckResult::ParseError => "parse_error",
            },
            "detail": check_result.to_string(),
        }));
    }

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "checks": results
            }))?
        );
    } else {
        for r in &results {
            let icon = match r["status"].as_str().unwrap_or("") {
                "current" => "✓",
                "outdated" => "⚠",
                "not_installed" | "not_found" => "✗",
                _ => "?",
            };
            println!(
                "{icon} {} — {}",
                r["slug"].as_str().unwrap_or(""),
                r["detail"].as_str().unwrap_or("")
            );
        }
    }

    Ok(())
}

async fn run_refresh(args: SkillRefreshArgs) -> anyhow::Result<()> {
    use altevra_adapters::{ClaudeCodeAdapter, ToolAdapter};

    let skills_dir = args.vault.join("06-skills");
    let skill_file = skills_dir.join(format!("{}.md", args.slug));

    if !skill_file.exists() {
        anyhow::bail!(
            "Skill '{}' not found in vault (looked at {})",
            args.slug,
            skill_file.display()
        );
    }

    let content = std::fs::read_to_string(&skill_file)?;
    let skill = altevra_skills::parser::parse_skill(&content)
        .map_err(|e| anyhow::anyhow!("Failed to parse skill '{}': {e}", args.slug))?;

    // Folder-per-skill layout: .claude/skills/<slug>/SKILL.md
    let dest = std::path::Path::new(".claude/skills")
        .join(&args.slug)
        .join("SKILL.md");

    // Drift protection: if file exists without managed header, refuse to overwrite.
    if dest.exists() {
        let existing = std::fs::read_to_string(&dest).unwrap_or_default();
        if !existing.contains("ALTEVRA_MANAGED: true") {
            anyhow::bail!(
                "Drift detected: {} exists but is not managed by Altevra. Remove it manually if you want to refresh.",
                dest.display()
            );
        }
    }

    let adapter = ClaudeCodeAdapter::new();
    let files = adapter.render_skills(vec![&skill])?;
    let rendered = files
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("Adapter returned no files for skill '{}'", args.slug))?;

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&dest, &rendered.content)?;

    println!("Refreshed: {} → {}", skill_file.display(), dest.display());
    Ok(())
}

pub fn load_skills_from_dir(registry: &mut SkillRegistry, dir: &Path) -> anyhow::Result<usize> {
    let mut count = 0;
    if !dir.exists() {
        return Ok(0);
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map(|e| e == "md").unwrap_or(false) {
            let content = std::fs::read_to_string(&path)?;
            if let Ok(_parsed) = parse_skill(&content) {
                let path_str = path.display().to_string();
                if registry.register(&path_str, &content).is_ok() {
                    count += 1;
                }
            }
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_skill_file(dir: &Path, slug: &str, version: &str) {
        let content = format!(
            "---\nslug: {slug}\nversion: {version}\ntitle: Test Skill {slug}\n---\n\nBody here."
        );
        std::fs::write(dir.join(format!("{slug}.md")), content).unwrap();
    }

    #[tokio::test]
    async fn test_skill_list_empty() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("06-skills")).unwrap();
        let args = SkillListArgs {
            json: true,
            vault: tmp.path().to_path_buf(),
        };
        run_list(args).await.unwrap();
    }

    #[tokio::test]
    async fn test_skill_list_with_skills() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join("06-skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        make_skill_file(&skills_dir, "altevra-core", "0.5.0");
        make_skill_file(&skills_dir, "test-skill", "1.2.3");

        let args = SkillListArgs {
            json: true,
            vault: tmp.path().to_path_buf(),
        };
        run_list(args).await.unwrap();
    }

    #[tokio::test]
    async fn test_skill_check_all() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join("06-skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        make_skill_file(&skills_dir, "altevra-core", "0.5.0");

        let args = SkillCheckArgs {
            all: true,
            slug: None,
            installed_version: Some("0.5.0".to_string()),
            vault: tmp.path().to_path_buf(),
            json: true,
        };
        run_check(args).await.unwrap();
    }

    #[tokio::test]
    async fn test_skill_refresh_missing_slug_errors() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("06-skills")).unwrap();
        let args = SkillRefreshArgs {
            slug: "nonexistent-skill".to_string(),
            vault: tmp.path().to_path_buf(),
        };
        assert!(run_refresh(args).await.is_err());
    }

    #[tokio::test]
    async fn test_skill_refresh_writes_file() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join("06-skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        make_skill_file(&skills_dir, "altevra-core", "0.5.0");

        // Change to tmp dir so .claude/skills/ is created there.
        let orig_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let args = SkillRefreshArgs {
            slug: "altevra-core".to_string(),
            vault: tmp.path().to_path_buf(),
        };
        let result = run_refresh(args).await;
        std::env::set_current_dir(orig_dir).unwrap();

        assert!(result.is_ok());
        // Folder-per-skill layout.
        assert!(tmp
            .path()
            .join(".claude/skills/altevra-core/SKILL.md")
            .exists());
    }
}

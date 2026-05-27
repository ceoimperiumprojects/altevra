use altevra_adapters::{ClaudeCodeAdapter, ToolAdapter};
use clap::Args;
use std::path::PathBuf;

#[derive(Args)]
pub struct ConnectArgs {
    /// Tool to connect (e.g. claude-code)
    #[arg(long)]
    pub tool: String,

    /// Project name
    #[arg(long)]
    pub project: Option<String>,

    /// Preview what would be installed without writing files
    #[arg(long)]
    pub dry_run: bool,

    /// Repository path (defaults to current directory)
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

pub async fn run(args: ConnectArgs) -> anyhow::Result<()> {
    let repo = args
        .repo
        .canonicalize()
        .unwrap_or_else(|_| args.repo.clone());
    let tool = args.tool.as_str();

    let adapter: Box<dyn ToolAdapter> = match tool {
        "claude-code" => Box::new(ClaudeCodeAdapter::new()),
        other => {
            anyhow::bail!(
                "Unknown tool: '{}'. Supported: claude-code\n\
                 Other tools (codex, cursor, aider) — adapters coming soon.",
                other
            );
        }
    };

    // Detect tool presence
    let detection = adapter.detect(&repo);

    // Build plan, then set dry_run from CLI flag
    let mut plan = adapter.build_install_plan(&repo, args.project.as_deref())?;
    plan.dry_run = args.dry_run;

    if args.dry_run {
        // Preview only — do not write
        if args.json {
            let output = serde_json::json!({
                "tool": tool,
                "project": args.project,
                "repo": repo.display().to_string(),
                "dry_run": true,
                "detection": {
                    "detected": detection.detected,
                    "notes": detection.notes,
                },
                "plan": {
                    "files_to_create": plan.files_to_create.iter().map(|f| serde_json::json!({
                        "path": f.path.display().to_string(),
                        "action": f.action,
                        "managed": f.managed,
                        "reason": f.reason,
                    })).collect::<Vec<_>>(),
                    "files_to_update": plan.files_to_update.iter().map(|f| serde_json::json!({
                        "path": f.path.display().to_string(),
                        "action": f.action,
                        "managed": f.managed,
                    })).collect::<Vec<_>>(),
                    "files_drifted": plan.files_drifted.iter().map(|f| serde_json::json!({
                        "path": f.path.display().to_string(),
                        "action": f.action,
                        "reason": f.reason,
                    })).collect::<Vec<_>>(),
                },
                "generated_files_preview": generate_file_previews(&*adapter, &repo, args.project.as_deref()),
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        } else {
            println!("Tool: {tool}");
            println!("Repo: {}", repo.display());
            if let Some(p) = &args.project {
                println!("Project: {p}");
            }
            println!("Mode: dry-run (preview only)");
            println!();

            if detection.detected {
                println!("Detection: Tool detected in repo");
                for note in &detection.notes {
                    println!("  - {note}");
                }
            } else {
                println!("Detection: Tool not yet configured in this repo");
            }
            println!();

            if !plan.files_to_create.is_empty() {
                println!("Files to create:");
                for f in &plan.files_to_create {
                    println!("  + {}", f.path.display());
                    if let Some(r) = &f.reason {
                        println!("    {r}");
                    }
                }
            }
            if !plan.files_to_update.is_empty() {
                println!("Files to update:");
                for f in &plan.files_to_update {
                    println!("  ~ {}", f.path.display());
                }
            }
            if !plan.files_drifted.is_empty() {
                println!("Files with drift (WILL NOT overwrite):");
                for f in &plan.files_drifted {
                    println!("  ⚠ {}", f.path.display());
                    if let Some(r) = &f.reason {
                        println!("    {r}");
                    }
                }
            }
            println!();
            println!("Dry-run only: run without --dry-run to install.");
        }
    } else {
        // Real install
        let result = adapter.install(&plan, &repo)?;

        if args.json {
            let output = serde_json::json!({
                "tool": tool,
                "project": args.project,
                "repo": repo.display().to_string(),
                "dry_run": false,
                "success": result.success,
                "files_created": result.files_created.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
                "files_updated": result.files_updated.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
                "files_skipped": result.files_skipped.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
                "error": result.error,
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        } else {
            println!("Tool: {tool}");
            println!("Repo: {}", repo.display());
            if let Some(p) = &args.project {
                println!("Project: {p}");
            }
            println!("Mode: install");
            println!();

            let nothing = result.files_created.is_empty()
                && result.files_updated.is_empty()
                && result.files_skipped.is_empty();
            if nothing {
                println!("Nothing to install — already up to date.");
            }
            if !result.files_created.is_empty() {
                println!("Created:");
                for f in &result.files_created {
                    println!("  + {}", f.display());
                }
            }
            if !result.files_updated.is_empty() {
                println!("Updated:");
                for f in &result.files_updated {
                    println!("  ~ {}", f.display());
                }
            }
            if !result.files_skipped.is_empty() {
                println!("Skipped (drift — manual edits detected):");
                for f in &result.files_skipped {
                    println!("  ⚠ {}", f.display());
                    println!("    To reset: delete the file and re-run connect.");
                }
            }
            if result.success {
                println!();
                println!("Done. Run: altevra agent bootstrap --tool {tool} --json");
            }
        }
    }

    Ok(())
}

fn generate_file_previews(
    adapter: &dyn ToolAdapter,
    repo: &std::path::Path,
    project: Option<&str>,
) -> serde_json::Value {
    use altevra_adapters::InstructionRenderInput;
    let input = InstructionRenderInput {
        tool_name: adapter.tool_name().to_string(),
        project: project.map(String::from),
        repo_path: repo.to_path_buf(),
        altevra_version: "0.1.0".to_string(),
    };

    match adapter.render_instructions(input) {
        Ok(files) => {
            serde_json::json!(files
                .iter()
                .map(|f| serde_json::json!({
                    "path": f.path.display().to_string(),
                    "managed": f.managed,
                    "checksum": f.checksum,
                    "content_preview": &f.content[..f.content.len().min(200)],
                }))
                .collect::<Vec<_>>())
        }
        Err(e) => serde_json::json!({"error": e.to_string()}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_connect_dry_run_json() {
        let tmp = TempDir::new().unwrap();
        let args = ConnectArgs {
            tool: "claude-code".to_string(),
            project: Some("altevra".to_string()),
            dry_run: true,
            repo: tmp.path().to_path_buf(),
            json: true,
        };
        run(args).await.unwrap();
        // dry_run must not write any files
        assert!(!tmp.path().join(".claude").exists());
    }

    #[tokio::test]
    async fn test_connect_real_install_writes_files() {
        let tmp = TempDir::new().unwrap();
        let args = ConnectArgs {
            tool: "claude-code".to_string(),
            project: Some("altevra".to_string()),
            dry_run: false,
            repo: tmp.path().to_path_buf(),
            json: false,
        };
        run(args).await.unwrap();
        assert!(tmp.path().join(".claude/altevra-instructions.md").exists());
        assert!(tmp.path().join(".claude/settings.json").exists());
        let content =
            std::fs::read_to_string(tmp.path().join(".claude/altevra-instructions.md")).unwrap();
        assert!(content.contains("ALTEVRA_MANAGED: true"));
    }

    #[tokio::test]
    async fn test_connect_unknown_tool_errors() {
        let tmp = TempDir::new().unwrap();
        let args = ConnectArgs {
            tool: "nonexistent-tool".to_string(),
            project: None,
            dry_run: true,
            repo: tmp.path().to_path_buf(),
            json: false,
        };
        assert!(run(args).await.is_err());
    }

    #[tokio::test]
    async fn test_connect_detects_claude_dir() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join(".claude")).unwrap();
        let args = ConnectArgs {
            tool: "claude-code".to_string(),
            project: Some("test".to_string()),
            dry_run: true,
            repo: tmp.path().to_path_buf(),
            json: true,
        };
        run(args).await.unwrap();
    }

    #[tokio::test]
    async fn test_generated_files_have_managed_header() {
        let adapter = ClaudeCodeAdapter::new();
        let input = altevra_adapters::InstructionRenderInput {
            tool_name: "claude-code".to_string(),
            project: Some("test".to_string()),
            repo_path: std::path::PathBuf::from("."),
            altevra_version: "0.1.0".to_string(),
        };
        let files = adapter.render_instructions(input).unwrap();
        assert!(!files.is_empty());
        assert!(files[0].content.contains("ALTEVRA_MANAGED: true"));
        assert!(files[0].managed);
    }

    fn write_skill(dir: &std::path::Path, slug: &str, version: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join(format!("{slug}.md")),
            format!("---\nslug: {slug}\nversion: {version}\ntitle: Test Skill\n---\nBody.\n"),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn test_connect_install_creates_skill_file() {
        let tmp = TempDir::new().unwrap();
        write_skill(&tmp.path().join("06-skills"), "altevra-core", "0.5.0");

        let args = ConnectArgs {
            tool: "claude-code".to_string(),
            project: Some("altevra".to_string()),
            dry_run: false,
            repo: tmp.path().to_path_buf(),
            json: false,
        };
        run(args).await.unwrap();

        let skill_dest = tmp.path().join(".claude/skills/altevra-core.md");
        assert!(
            skill_dest.exists(),
            ".claude/skills/altevra-core.md must be created"
        );
        let content = std::fs::read_to_string(&skill_dest).unwrap();
        assert!(
            content.contains("altevra-core"),
            "skill file must contain slug"
        );
    }

    #[tokio::test]
    async fn test_connect_skill_dry_run_does_not_write_skill() {
        let tmp = TempDir::new().unwrap();
        write_skill(&tmp.path().join("06-skills"), "altevra-core", "0.5.0");

        let args = ConnectArgs {
            tool: "claude-code".to_string(),
            project: Some("altevra".to_string()),
            dry_run: true,
            repo: tmp.path().to_path_buf(),
            json: false,
        };
        run(args).await.unwrap();
        assert!(!tmp.path().join(".claude").exists());
    }

    #[tokio::test]
    async fn test_connect_skill_drift_protection() {
        let tmp = TempDir::new().unwrap();
        write_skill(&tmp.path().join("06-skills"), "altevra-core", "0.5.0");

        // First install
        let args = ConnectArgs {
            tool: "claude-code".to_string(),
            project: Some("altevra".to_string()),
            dry_run: false,
            repo: tmp.path().to_path_buf(),
            json: false,
        };
        run(args).await.unwrap();

        // Simulate manual edit — strip managed header
        let skill_dest = tmp.path().join(".claude/skills/altevra-core.md");
        std::fs::write(&skill_dest, "# manually edited — no header\n").unwrap();

        // Second install — must NOT overwrite
        let args2 = ConnectArgs {
            tool: "claude-code".to_string(),
            project: Some("altevra".to_string()),
            dry_run: false,
            repo: tmp.path().to_path_buf(),
            json: false,
        };
        run(args2).await.unwrap();

        let content = std::fs::read_to_string(&skill_dest).unwrap();
        assert!(
            content.contains("manually edited"),
            "drifted skill file must not be overwritten"
        );
    }
}

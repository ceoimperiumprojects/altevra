use altevra_adapters::{
    AntigravityAdapter, ClaudeCodeAdapter, CodexAdapter, CursorAdapter, ToolAdapter,
};
use clap::Args;
use std::path::PathBuf;

#[derive(Args, Clone)]
pub struct ConnectArgs {
    /// Tool to connect (claude-code, codex, cursor, antigravity)
    #[arg(long)]
    pub tool: String,

    /// Project name
    #[arg(long)]
    pub project: Option<String>,

    /// Preview what would be installed without writing files
    #[arg(long)]
    pub dry_run: bool,

    /// Force overwrite of drifted files
    #[arg(long)]
    pub force: bool,

    /// Repository path (defaults to current directory)
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,
}

pub fn resolve_adapter(tool: &str) -> anyhow::Result<Box<dyn ToolAdapter>> {
    let adapter: Box<dyn ToolAdapter> = match tool {
        "claude-code" => Box::new(ClaudeCodeAdapter::new()),
        "codex" => Box::new(CodexAdapter::new()),
        "cursor" => Box::new(CursorAdapter::new()),
        "antigravity" => Box::new(AntigravityAdapter::new()),
        other => anyhow::bail!(
            "Unknown tool: '{}'. Supported: claude-code, codex, cursor, antigravity",
            other
        ),
    };
    Ok(adapter)
}

pub async fn run(args: ConnectArgs) -> anyhow::Result<()> {
    let repo = args
        .repo
        .canonicalize()
        .unwrap_or_else(|_| args.repo.clone());
    let tool = args.tool.as_str();

    let adapter = resolve_adapter(tool)?;
    let detection = adapter.detect(&repo);
    let mut plan = adapter.build_install_plan(&repo, args.project.as_deref())?;
    plan.dry_run = args.dry_run;

    if args.dry_run {
        println!("Tool: {tool}");
        println!("Repo: {}", repo.display());
        if let Some(p) = &args.project {
            println!("Project: {p}");
        }
        println!("Mode: dry-run");
        println!();

        if detection.detected {
            println!("Detection: Tool detected in repo");
            for note in &detection.notes {
                println!("  - {note}");
            }
        } else {
            println!("Detection: Tool not yet configured");
        }
        println!();

        if !plan.files_to_create.is_empty() {
            println!("Files to create:");
            for f in &plan.files_to_create {
                println!("  + {}", f.path.display());
            }
        }
        if !plan.files_to_update.is_empty() {
            println!("Files to update:");
            for f in &plan.files_to_update {
                println!("  ~ {}", f.path.display());
            }
        }
        if !plan.files_drifted.is_empty() {
            println!("Files with drift (won't overwrite without --force):");
            for f in &plan.files_drifted {
                println!("  ⚠ {}", f.path.display());
            }
        }

        return Ok(());
    }

    // If forcing, clear drifted file paths first
    if args.force && !plan.files_drifted.is_empty() {
        for f in &plan.files_drifted {
            let full = repo.join(&f.path);
            let _ = std::fs::remove_file(&full);
        }
        plan = adapter.build_install_plan(&repo, args.project.as_deref())?;
    }

    let result = adapter.install(&plan, &repo)?;

    println!("Tool: {tool}");
    println!("Mode: install");
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
        println!("Skipped (drift):");
        for f in &result.files_skipped {
            println!("  ⚠ {}", f.display());
        }
    }
    if result.success {
        println!("\nDone. Run: altevra agent bootstrap --tool {tool} --json");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn dry_run_claude_code() {
        let tmp = TempDir::new().unwrap();
        let args = ConnectArgs {
            tool: "claude-code".to_string(),
            project: Some("altevra".to_string()),
            dry_run: true,
            force: false,
            repo: tmp.path().to_path_buf(),
        };
        run(args).await.unwrap();
        assert!(!tmp.path().join(".claude").exists());
    }

    #[tokio::test]
    async fn install_claude_code() {
        let tmp = TempDir::new().unwrap();
        let args = ConnectArgs {
            tool: "claude-code".to_string(),
            project: Some("altevra".to_string()),
            dry_run: false,
            force: false,
            repo: tmp.path().to_path_buf(),
        };
        run(args).await.unwrap();
        assert!(tmp.path().join(".claude/altevra-instructions.md").exists());
    }

    #[tokio::test]
    async fn dry_run_codex() {
        let tmp = TempDir::new().unwrap();
        let args = ConnectArgs {
            tool: "codex".to_string(),
            project: Some("altevra".to_string()),
            dry_run: true,
            force: false,
            repo: tmp.path().to_path_buf(),
        };
        run(args).await.unwrap();
    }

    #[tokio::test]
    async fn dry_run_cursor() {
        let tmp = TempDir::new().unwrap();
        let args = ConnectArgs {
            tool: "cursor".to_string(),
            project: None,
            dry_run: true,
            force: false,
            repo: tmp.path().to_path_buf(),
        };
        run(args).await.unwrap();
    }

    #[tokio::test]
    async fn dry_run_antigravity() {
        let tmp = TempDir::new().unwrap();
        let args = ConnectArgs {
            tool: "antigravity".to_string(),
            project: None,
            dry_run: true,
            force: false,
            repo: tmp.path().to_path_buf(),
        };
        run(args).await.unwrap();
    }

    #[tokio::test]
    async fn unknown_tool_errors() {
        let tmp = TempDir::new().unwrap();
        let args = ConnectArgs {
            tool: "nonexistent".to_string(),
            project: None,
            dry_run: true,
            force: false,
            repo: tmp.path().to_path_buf(),
        };
        assert!(run(args).await.is_err());
    }
}

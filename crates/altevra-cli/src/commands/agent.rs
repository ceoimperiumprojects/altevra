use altevra_bootstrap::{BootstrapBuilder, SetupStatus};
use altevra_skills::registry::SkillRegistry;
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum AgentCommands {
    /// Get bootstrap packet for agent session start
    Bootstrap(AgentBootstrapArgs),
    /// Show agent setup status
    Status(AgentStatusArgs),
    /// Show agent instructions for a tool
    Instructions(AgentInstructionsArgs),
}

#[derive(Args)]
pub struct AgentBootstrapArgs {
    /// Tool name
    #[arg(long, default_value = "claude-code")]
    pub tool: String,

    /// Project name
    #[arg(long)]
    pub project: Option<String>,

    /// Currently installed altevra-core skill version
    #[arg(long)]
    pub installed_skill_version: Option<String>,

    /// Session ID (auto-generated if not provided)
    #[arg(long)]
    pub session_id: Option<String>,

    /// Output as JSON (required for machine consumption)
    #[arg(long)]
    pub json: bool,

    /// Vault path to load skills from
    #[arg(long, default_value = ".")]
    pub vault: PathBuf,

    /// Brain database — source of the tool register + session-context block.
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
}

#[derive(Args)]
pub struct AgentStatusArgs {
    #[arg(long, default_value = "claude-code")]
    pub tool: String,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct AgentInstructionsArgs {
    #[arg(long, default_value = "claude-code")]
    pub tool: String,
    #[arg(long)]
    pub project: Option<String>,
}

pub async fn run(cmd: AgentCommands) -> anyhow::Result<()> {
    match cmd {
        AgentCommands::Bootstrap(args) => run_bootstrap(args).await,
        AgentCommands::Status(args) => run_status(args).await,
        AgentCommands::Instructions(args) => run_instructions(args).await,
    }
}

async fn run_bootstrap(args: AgentBootstrapArgs) -> anyhow::Result<()> {
    // Load skill registry from vault
    let mut registry = SkillRegistry::new();
    let skills_dir = args.vault.join("06-skills");
    if skills_dir.exists() {
        crate::commands::skill::load_skills_from_dir(&mut registry, &skills_dir)?;
    }

    // Check skill freshness
    let freshness = vec![altevra_bootstrap::freshness::FreshnessCheck::check(
        &registry,
        "altevra-core",
        args.installed_skill_version.as_deref(),
    )];

    // Build setup status
    let setup = SetupStatus::placeholder(&args.tool);

    // §P2 #7: tool register + the gated session-context block ride the packet.
    // Fault-tolerant — a locked/missing DB degrades to an empty register.
    let (available_tools, session_context) = {
        let run = async {
            let pool = altevra_db::create_pool(&args.db.to_string_lossy()).await?;
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
    };

    let mut builder = BootstrapBuilder::new(&args.tool, env!("CARGO_PKG_VERSION"))
        .skill_freshness(freshness)
        .setup_status(setup)
        .available_tools(available_tools)
        .session_context(session_context);

    if let Some(p) = &args.project {
        builder = builder.project(p.clone());
    }

    let packet = builder.build();

    if args.json {
        println!("{}", serde_json::to_string_pretty(&packet)?);
    } else {
        println!("=== Altevra Agent Bootstrap Packet ===");
        println!("Tool:     {}", packet.tool_name);
        println!(
            "Project:  {}",
            packet.project.as_deref().unwrap_or("(none)")
        );
        println!("Version:  {}", packet.altevra_version);
        println!("Session:  {}", packet.session_id);
        println!();

        println!("Skill Freshness:");
        for f in &packet.skill_freshness {
            println!(
                "  {} — {} (installed: {}, latest: {})",
                f.skill_slug,
                f.status,
                f.installed_version.as_deref().unwrap_or("none"),
                f.latest_version.as_deref().unwrap_or("unknown"),
            );
        }

        println!();
        println!("Last Updates: {} items", packet.last_updates.len());

        if !packet.warnings.is_empty() {
            println!();
            println!("Warnings:");
            for w in &packet.warnings {
                println!("  ⚠ {w}");
            }
        }

        if let Some(action) = &packet.recommended_next_action {
            println!();
            println!("Recommended Next Action: {action}");
        }
    }

    Ok(())
}

async fn run_status(args: AgentStatusArgs) -> anyhow::Result<()> {
    use altevra_bootstrap::setup_status::{ComponentCheck, ComponentStatus, SetupStatus};

    let repo = std::path::Path::new(".");

    let vault_ok = repo.join(".altevra/config.toml").exists();
    let skills_ok = repo.join("06-skills").is_dir()
        && std::fs::read_dir(repo.join("06-skills"))
            .map(|d| {
                d.flatten()
                    .any(|e| e.path().extension().map(|x| x == "md").unwrap_or(false))
            })
            .unwrap_or(false);
    let claude_instructions_ok = repo.join(".claude/altevra-instructions.md").exists();
    let claude_settings_ok = repo.join(".claude/settings.json").exists();
    let skills_installed_ok = repo.join(".claude/skills").is_dir()
        && std::fs::read_dir(repo.join(".claude/skills"))
            .map(|d| {
                d.flatten()
                    .any(|e| e.path().extension().map(|x| x == "md").unwrap_or(false))
            })
            .unwrap_or(false);

    let mk = |component: &str, ok: bool, path: Option<&str>, fix: &str| ComponentCheck {
        component: component.into(),
        status: if ok {
            ComponentStatus::Current
        } else {
            ComponentStatus::Missing
        },
        path: path.map(Into::into),
        note: if ok { None } else { Some(fix.into()) },
    };

    let components = vec![
        mk(
            "vault",
            vault_ok,
            Some(".altevra/config.toml"),
            "Run: altevra init",
        ),
        mk(
            "skills",
            skills_ok,
            Some("06-skills/"),
            "Add skills to 06-skills/",
        ),
        mk(
            "instruction_file",
            claude_instructions_ok,
            Some(".claude/altevra-instructions.md"),
            "Run: altevra connect --tool claude-code",
        ),
        mk(
            "settings_json",
            claude_settings_ok,
            Some(".claude/settings.json"),
            "Run: altevra connect --tool claude-code",
        ),
        mk(
            "skills_installed",
            skills_installed_ok,
            Some(".claude/skills/"),
            "Run: altevra connect --tool claude-code",
        ),
    ];

    let all_ok = components
        .iter()
        .all(|c| matches!(c.status, ComponentStatus::Current));
    let any_ok = components
        .iter()
        .any(|c| matches!(c.status, ComponentStatus::Current));

    let overall = if all_ok {
        ComponentStatus::Current
    } else if any_ok {
        ComponentStatus::Outdated
    } else {
        ComponentStatus::Missing
    };

    let status = SetupStatus {
        tool_name: args.tool.clone(),
        overall,
        components,
        warnings: vec![],
        run_repair: !all_ok,
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        println!("Setup Status: {}", args.tool);
        println!("Overall: {}", status.overall);
        for c in &status.components {
            let icon = if matches!(c.status, ComponentStatus::Current) {
                "✓"
            } else {
                "✗"
            };
            println!("  {icon} {} — {}", c.component, c.status);
            if let Some(note) = &c.note {
                println!("    {note}");
            }
        }
    }

    Ok(())
}

async fn run_instructions(args: AgentInstructionsArgs) -> anyhow::Result<()> {
    use altevra_adapters::{ClaudeCodeAdapter, InstructionRenderInput, ToolAdapter};

    let adapter: Box<dyn ToolAdapter> = match args.tool.as_str() {
        "claude-code" => Box::new(ClaudeCodeAdapter::new()),
        other => anyhow::bail!("Unknown tool: {other}"),
    };

    let input = InstructionRenderInput {
        tool_name: adapter.tool_name().to_string(),
        project: args.project.clone(),
        repo_path: std::path::PathBuf::from("."),
        altevra_version: env!("CARGO_PKG_VERSION").to_string(),
    };

    let files = adapter.render_instructions(input)?;
    for f in &files {
        println!("--- {} ---", f.path.display());
        println!("{}", f.content);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_bootstrap_json_output() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("06-skills")).unwrap();

        let args = AgentBootstrapArgs {
            tool: "claude-code".to_string(),
            project: Some("altevra".to_string()),
            installed_skill_version: None,
            session_id: None,
            json: true,
            vault: tmp.path().to_path_buf(),
            db: tmp.path().join("agent-test.db"),
        };
        run_bootstrap(args).await.unwrap();
    }

    #[tokio::test]
    async fn test_bootstrap_with_skill() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join("06-skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        std::fs::write(
            skills_dir.join("altevra-core.md"),
            "---\nslug: altevra-core\nversion: 0.5.0\ntitle: Altevra Core\n---\nBody.",
        )
        .unwrap();

        let args = AgentBootstrapArgs {
            tool: "claude-code".to_string(),
            project: Some("altevra".to_string()),
            installed_skill_version: Some("0.5.0".to_string()),
            session_id: None,
            json: true,
            vault: tmp.path().to_path_buf(),
            db: tmp.path().join("agent-test.db"),
        };
        run_bootstrap(args).await.unwrap();
    }

    #[tokio::test]
    async fn test_bootstrap_packet_fields_present() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("06-skills")).unwrap();

        // Capture output by running in a way that returns the packet directly
        let registry = SkillRegistry::new();
        let freshness = vec![altevra_bootstrap::freshness::FreshnessCheck::check(
            &registry,
            "altevra-core",
            None,
        )];
        let packet = BootstrapBuilder::new("claude-code", "0.1.0")
            .project("test")
            .skill_freshness(freshness)
            .build();

        let json_str = serde_json::to_string(&packet).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        // Check all required fields are present
        assert!(parsed.get("tool_name").is_some());
        assert!(parsed.get("project").is_some());
        assert!(parsed.get("skill_freshness").is_some());
        assert!(parsed.get("setup_status").is_some());
        assert!(parsed.get("last_updates").is_some());
        assert!(parsed.get("warnings").is_some());
        assert!(parsed.get("session_id").is_some());
    }

    #[tokio::test]
    async fn test_bootstrap_skill_current_when_version_matches() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join("06-skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        std::fs::write(
            skills_dir.join("altevra-core.md"),
            "---\nslug: altevra-core\nversion: 0.5.0\ntitle: Altevra Core\n---\nBody.",
        )
        .unwrap();

        let mut registry = SkillRegistry::new();
        crate::commands::skill::load_skills_from_dir(&mut registry, &skills_dir).unwrap();

        let freshness = vec![altevra_bootstrap::freshness::FreshnessCheck::check(
            &registry,
            "altevra-core",
            Some("0.5.0"),
        )];
        let packet = BootstrapBuilder::new("claude-code", "0.1.0")
            .skill_freshness(freshness)
            .build();

        assert_eq!(packet.skill_freshness.len(), 1);
        assert_eq!(
            packet.skill_freshness[0].status,
            altevra_bootstrap::freshness::SkillFreshnessStatus::Current
        );
        assert!(packet.skill_freshness[0].action_required.is_none());
    }

    #[tokio::test]
    async fn test_bootstrap_skill_outdated_when_version_behind() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join("06-skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        std::fs::write(
            skills_dir.join("altevra-core.md"),
            "---\nslug: altevra-core\nversion: 0.5.0\ntitle: Altevra Core\n---\nBody.",
        )
        .unwrap();

        let mut registry = SkillRegistry::new();
        crate::commands::skill::load_skills_from_dir(&mut registry, &skills_dir).unwrap();

        let freshness = vec![altevra_bootstrap::freshness::FreshnessCheck::check(
            &registry,
            "altevra-core",
            Some("0.4.0"),
        )];
        let packet = BootstrapBuilder::new("claude-code", "0.1.0")
            .skill_freshness(freshness)
            .build();

        assert_eq!(
            packet.skill_freshness[0].status,
            altevra_bootstrap::freshness::SkillFreshnessStatus::Outdated
        );
        assert!(packet.skill_freshness[0].action_required.is_some());
    }
}

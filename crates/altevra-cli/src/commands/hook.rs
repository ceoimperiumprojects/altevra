use altevra_core::updates::{Importance, UpdateFeedItem};
use altevra_hooks::{HookRegistry, HookRunContext, HookRunner};
use clap::{Args, Subcommand};
use uuid::Uuid;

#[derive(Subcommand)]
pub enum HookCommands {
    /// List all registered hooks
    List(HookListArgs),
    /// Run a hook
    Run(HookRunArgs),
    /// Show hook status
    Status(HookStatusArgs),
}

#[derive(Args)]
pub struct HookListArgs {
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct HookRunArgs {
    /// Hook slug to run (e.g. session_start)
    pub slug: String,
    /// Tool name
    #[arg(long, default_value = "unknown")]
    pub tool: String,
    /// Project name
    #[arg(long)]
    pub project: Option<String>,
    /// Session ID
    #[arg(long)]
    pub session_id: Option<String>,
    /// JSON payload
    #[arg(long)]
    pub payload: Option<String>,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct HookStatusArgs {
    pub slug: Option<String>,
    #[arg(long)]
    pub json: bool,
}

pub async fn run(cmd: HookCommands) -> anyhow::Result<()> {
    match cmd {
        HookCommands::List(args) => run_list(args).await,
        HookCommands::Run(args) => run_hook(args).await,
        HookCommands::Status(args) => run_status(args).await,
    }
}

async fn run_list(args: HookListArgs) -> anyhow::Result<()> {
    let registry = HookRegistry::with_defaults();
    let hooks = registry.list();

    if args.json {
        let items: Vec<_> = hooks
            .iter()
            .map(|h| {
                serde_json::json!({
                    "slug": h.slug,
                    "version": h.version,
                    "hook_type": h.hook_type.to_string(),
                    "enabled": h.enabled,
                    "actions": h.actions,
                    "description": h.description,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "hooks": items,
                "count": items.len()
            }))?
        );
    } else {
        println!("Hooks ({}):", hooks.len());
        for h in hooks {
            let status = if h.enabled { "enabled" } else { "disabled" };
            println!(
                "  {} v{} [{status}] — {}",
                h.slug,
                h.version,
                h.description.as_deref().unwrap_or("")
            );
            println!("    actions: {}", h.actions.join(", "));
        }
    }

    Ok(())
}

async fn run_hook(args: HookRunArgs) -> anyhow::Result<()> {
    let registry = HookRegistry::with_defaults();
    let runner = HookRunner::new(&registry);

    let payload: serde_json::Value = args
        .payload
        .as_deref()
        .and_then(|p| serde_json::from_str(p).ok())
        .unwrap_or_default();

    let ctx = HookRunContext {
        hook_slug: args.slug.clone(),
        tool_name: args.tool.clone(),
        project: args.project.clone(),
        session_id: args.session_id.clone(),
        payload,
    };

    let outcome = runner.run(ctx);

    // Emit local update event so `altevra updates` can show hook activity.
    if outcome.success {
        let event = UpdateFeedItem::from_event(
            Uuid::new_v4(),
            format!("hook.{}", args.slug),
            Importance::Low,
            format!("Hook ran: {}", args.slug),
            format!(
                "Tool: {} | Actions: {}",
                args.tool,
                outcome.actions_executed.join(", ")
            ),
        );
        crate::commands::updates::append_local_update(&event);
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&outcome)?);
    } else {
        let status = if outcome.success { "OK" } else { "FAILED" };
        println!(
            "[{status}] Hook '{}' ran in {}ms",
            outcome.hook_slug, outcome.duration_ms
        );
        if !outcome.actions_executed.is_empty() {
            println!(
                "  Actions executed: {}",
                outcome.actions_executed.join(", ")
            );
        }
        if let Some(err) = &outcome.error_message {
            println!("  Error: {err}");
        }
    }

    Ok(())
}

async fn run_status(args: HookStatusArgs) -> anyhow::Result<()> {
    let registry = HookRegistry::with_defaults();
    if let Some(slug) = &args.slug {
        match registry.get(slug) {
            Some(h) => {
                if args.json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "slug": h.slug,
                            "version": h.version,
                            "enabled": h.enabled,
                            "status": if h.enabled { "active" } else { "disabled" }
                        }))?
                    );
                } else {
                    println!(
                        "{} v{}: {}",
                        h.slug,
                        h.version,
                        if h.enabled { "active" } else { "disabled" }
                    );
                }
            }
            None => {
                anyhow::bail!("Hook not found: {slug}");
            }
        }
    } else {
        println!("Use 'altevra hook list' to see all hooks.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_hook_list_json() {
        let args = HookListArgs { json: true };
        run_list(args).await.unwrap();
    }

    #[tokio::test]
    async fn test_hook_run_session_start() {
        let args = HookRunArgs {
            slug: "session_start".to_string(),
            tool: "claude-code".to_string(),
            project: Some("altevra".to_string()),
            session_id: None,
            payload: None,
            json: true,
        };
        run_hook(args).await.unwrap();
    }

    #[tokio::test]
    async fn test_hook_run_session_end() {
        let args = HookRunArgs {
            slug: "session_end".to_string(),
            tool: "claude-code".to_string(),
            project: None,
            session_id: None,
            payload: None,
            json: true,
        };
        run_hook(args).await.unwrap();
    }

    #[tokio::test]
    async fn test_hook_run_unknown_hook() {
        let args = HookRunArgs {
            slug: "nonexistent_hook".to_string(),
            tool: "claude-code".to_string(),
            project: None,
            session_id: None,
            payload: None,
            json: true,
        };
        // Should succeed — runner returns error outcome, not panic
        run_hook(args).await.unwrap();
    }
}

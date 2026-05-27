mod commands;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser)]
#[command(
    name = "altevra",
    version = VERSION,
    about = "Agent OS — CLI-first, local-first, adapter-based context layer for AI tools",
    long_about = None
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Enable debug logging
    #[arg(long, global = true)]
    debug: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize Altevra in the current directory
    Init(commands::init::InitArgs),

    /// Show recent updates from the Altevra feed
    Updates(commands::updates::UpdatesArgs),

    /// Manage skills
    #[command(subcommand)]
    Skill(commands::skill::SkillCommands),

    /// Manage hooks
    #[command(subcommand)]
    Hook(commands::hook::HookCommands),

    /// Connect a tool to the current project
    Connect(commands::connect::ConnectArgs),

    /// Agent lifecycle commands
    #[command(subcommand)]
    Agent(commands::agent::AgentCommands),

    /// Start MCP server on stdio (JSON-RPC 2.0)
    Serve(commands::serve::ServeArgs),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let filter = if cli.debug {
        EnvFilter::new("debug")
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"))
    };
    tracing_subscriber::fmt().with_env_filter(filter).init();

    match cli.command {
        Commands::Init(args) => commands::init::run(args).await,
        Commands::Updates(args) => commands::updates::run(args).await,
        Commands::Skill(cmd) => commands::skill::run(cmd).await,
        Commands::Hook(cmd) => commands::hook::run(cmd).await,
        Commands::Connect(args) => commands::connect::run(args).await,
        Commands::Agent(cmd) => commands::agent::run(cmd).await,
        Commands::Serve(args) => commands::serve::run(args).await,
    }
}

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

    /// Tool setup management (alias of connect with verify/repair/status)
    #[command(subcommand)]
    Setup(commands::setup::SetupCommands),

    /// Agent lifecycle commands
    #[command(subcommand)]
    Agent(commands::agent::AgentCommands),

    /// Start MCP server on stdio (JSON-RPC 2.0)
    Serve(commands::serve::ServeArgs),

    /// Run health checks on the current Altevra setup
    Doctor(commands::doctor::DoctorArgs),

    /// Manage Altevra configuration
    #[command(subcommand)]
    Config(commands::config::ConfigCommands),

    /// Memory ingest / search / context
    #[command(subcommand)]
    Memory(commands::memory::MemoryCommands),

    /// Build a layered system prompt for an agent tool
    #[command(subcommand)]
    Prompt(commands::prompt::PromptCommands),

    /// Web research pipeline
    #[command(subcommand)]
    Research(commands::research::ResearchCommands),

    /// Secrets management (keyring or encrypted file)
    #[command(subcommand)]
    Secrets(commands::secrets::SecretsCommands),

    /// Project context report
    Context(commands::context::ContextArgs),

    /// Journal commands (today / generate)
    #[command(subcommand)]
    Journal(commands::journal::JournalCommands),

    /// Observer brain — detect patterns and emit insights
    #[command(subcommand)]
    Observer(commands::observer::ObserverCommands),

    /// Manage agent sessions (v0.3 omniscient recorder)
    #[command(subcommand)]
    Session(commands::session::SessionCommands),

    /// Record a single agent turn into the recorder
    Turn(commands::turn::TurnRecordArgs),

    /// Search across recorded turn content (BM25-style)
    TurnSearch(commands::turn_search::TurnSearchArgs),

    /// File change history (recorded by watcher/hooks)
    #[command(subcommand)]
    Files(commands::files::FilesCommands),

    /// Handle a tool hook event (reads JSON from stdin)
    HookHandle(commands::hook_handle::HookHandleArgs),

    /// File watcher daemon — emits FileChanged events and queues for embedding
    #[command(subcommand)]
    Watch(commands::watch::WatchCommands),

    /// Continuous embedder worker (drains pending_indexing → Gemini → vectors)
    #[command(subcommand)]
    Embed(commands::embed::EmbedCommands),

    /// Autonomous brain daemon (periodic jobs: observer, classifier, indexer, ...)
    #[command(subcommand)]
    Brain(commands::brain::BrainCommands),

    /// Print the Altevra banner / about screen
    Banner(commands::banner::BannerArgs),
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
        Commands::Setup(cmd) => commands::setup::run(cmd).await,
        Commands::Agent(cmd) => commands::agent::run(cmd).await,
        Commands::Serve(args) => commands::serve::run(args).await,
        Commands::Doctor(args) => commands::doctor::run(args).await,
        Commands::Config(cmd) => commands::config::run(cmd).await,
        Commands::Memory(cmd) => commands::memory::run(cmd).await,
        Commands::Prompt(cmd) => commands::prompt::run(cmd).await,
        Commands::Research(cmd) => commands::research::run(cmd).await,
        Commands::Secrets(cmd) => commands::secrets::run(cmd).await,
        Commands::Context(args) => commands::context::run(args).await,
        Commands::Journal(cmd) => commands::journal::run(cmd).await,
        Commands::Observer(cmd) => commands::observer::run(cmd).await,
        Commands::Session(cmd) => commands::session::run(cmd).await,
        Commands::Turn(args) => commands::turn::run(args).await,
        Commands::TurnSearch(args) => commands::turn_search::run(args).await,
        Commands::Files(cmd) => commands::files::run(cmd).await,
        Commands::HookHandle(args) => commands::hook_handle::run(args).await,
        Commands::Watch(cmd) => commands::watch::run(cmd).await,
        Commands::Embed(cmd) => commands::embed::run(cmd).await,
        Commands::Brain(cmd) => commands::brain::run(cmd).await,
        Commands::Banner(args) => commands::banner::run(args).await,
    }
}

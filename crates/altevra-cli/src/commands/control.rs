//! `altevra control` — the P0.3 control plane: human review queue, redaction
//! check, and the exposure-decision audit. Mutating verbs (review approve/reject)
//! pass through `require_human_presence` (R4 / HP) — an MCP/agent caller can never
//! reach them, and "approved" is never an input flag (HP-2): the decision is
//! recorded by the core AFTER a TTY/unlock presence check.

use altevra_core::presence::require_human_presence;
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum ControlCommands {
    /// Human review queue (list/show/approve/reject).
    Review(ReviewArgs),
    /// Scan stdin or a file for secrets/PII — reports kinds + counts, never raw.
    Redact(RedactArgs),
    /// Query the append-only exposure-decision audit.
    Audit(AuditArgs),
}

pub async fn run(cmd: ControlCommands) -> anyhow::Result<()> {
    match cmd {
        ControlCommands::Review(a) => run_review(a).await,
        ControlCommands::Redact(a) => run_redact(a).await,
        ControlCommands::Audit(a) => run_audit(a).await,
    }
}

// ---- review -----------------------------------------------------------------

#[derive(Args)]
pub struct ReviewArgs {
    #[command(subcommand)]
    pub action: ReviewAction,
    // global so it can appear before OR after the subcommand (e.g. `review list --db X`).
    #[arg(long, global = true, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
}

#[derive(Subcommand)]
pub enum ReviewAction {
    /// List review items (default: open only).
    List {
        #[arg(long)]
        all: bool,
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
    /// Show one review item.
    Show { id: String },
    /// Approve an item (requires human presence: TTY or ALTEVRA_UNLOCK).
    Approve { id: String },
    /// Reject an item (requires human presence).
    Reject { id: String },
}

async fn run_review(args: ReviewArgs) -> anyhow::Result<()> {
    let pool = altevra_db::create_pool(&args.db.to_string_lossy()).await?;
    altevra_db::run_migrations(&pool).await?;
    let repo = altevra_db::TasksRepository::new(&pool);

    match args.action {
        ReviewAction::List { all, limit } => {
            let status = if all { None } else { Some("open") };
            let items = repo.list_review_items(status, limit).await?;
            if items.is_empty() {
                println!("No review items{}.", if all { "" } else { " (open)" });
            } else {
                println!("Review items ({}):", items.len());
                for it in &items {
                    println!(
                        "  {} [{}] {} — {}",
                        &it.id.to_string()[..8],
                        it.status,
                        it.kind,
                        it.title
                    );
                }
            }
        }
        ReviewAction::Show { id } => {
            let uuid = parse_id(&id)?;
            match repo.get_review_item(uuid).await? {
                Some(it) => println!(
                    "{} [{}] {}\n  kind: {}\n  body: {}",
                    it.id,
                    it.status,
                    it.title,
                    it.kind,
                    it.body.as_deref().unwrap_or("")
                ),
                None => anyhow::bail!("review item not found: {id}"),
            }
        }
        ReviewAction::Approve { id } => decide(&repo, &id, "approved").await?,
        ReviewAction::Reject { id } => decide(&repo, &id, "rejected").await?,
    }
    Ok(())
}

async fn decide(
    repo: &altevra_db::TasksRepository<'_>,
    id: &str,
    decision: &str,
) -> anyhow::Result<()> {
    // HP gate: refuse unless a human is present (TTY or unlock token). An MCP/agent
    // caller has no path here at all (HP-1) — this verb lives only on the CLI.
    let proof = require_human_presence().map_err(|e| anyhow::anyhow!("{e}"))?;
    let uuid = parse_id(id)?;
    let decided_by = format!("pavle:{}", proof.method.as_str());
    let changed = repo.decide_review_item(uuid, decision, &decided_by).await?;
    if changed {
        println!(
            "review {} -> {decision} (by {decided_by})",
            &uuid.to_string()[..8]
        );
    } else {
        anyhow::bail!("review item {id} not found or not open");
    }
    Ok(())
}

fn parse_id(id: &str) -> anyhow::Result<uuid::Uuid> {
    uuid::Uuid::parse_str(id).map_err(|_| anyhow::anyhow!("invalid review id: {id}"))
}

// ---- redact check -----------------------------------------------------------

#[derive(Args)]
pub struct RedactArgs {
    #[command(subcommand)]
    pub action: RedactAction,
}

#[derive(Subcommand)]
pub enum RedactAction {
    /// Scan stdin (or --file) and report what WOULD be redacted (no raw values).
    Check {
        #[arg(long)]
        file: Option<PathBuf>,
    },
}

async fn run_redact(args: RedactArgs) -> anyhow::Result<()> {
    let RedactAction::Check { file } = args.action;
    let input = match file {
        Some(p) => std::fs::read_to_string(&p)?,
        None => {
            use std::io::Read;
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf)?;
            buf
        }
    };
    let guarded = altevra_secrets::guard_text(&input, altevra_core::Sensitivity::Internal);
    println!("redaction_status: {}", guarded.redaction_status);
    println!("sensitivity:      {}", guarded.sensitivity);
    if guarded.risk_tags.is_empty() {
        println!("risk_tags:        (none)");
    } else {
        let tags: Vec<String> = guarded.risk_tags.iter().map(|t| t.to_string()).collect();
        println!("risk_tags:        {}", tags.join(", "));
    }
    if guarded.sightings.is_empty() {
        println!("secrets:          none detected");
    } else {
        println!("secrets ({}):", guarded.sightings.len());
        for s in &guarded.sightings {
            // fingerprint + kind ONLY — never the raw value.
            println!("  - {} [{}] fp:{}", s.secret_kind, s.action, s.fingerprint);
        }
    }
    Ok(())
}

// ---- audit query ------------------------------------------------------------

#[derive(Args)]
pub struct AuditArgs {
    #[command(subcommand)]
    pub action: AuditAction,
    #[arg(long, global = true, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
}

#[derive(Subcommand)]
pub enum AuditAction {
    /// Query recent exposure decisions (append-only audit).
    Query {
        #[arg(long, default_value_t = 20)]
        limit: i64,
    },
}

async fn run_audit(args: AuditArgs) -> anyhow::Result<()> {
    use sqlx::Row;
    let AuditAction::Query { limit } = args.action;
    let pool = altevra_db::create_pool(&args.db.to_string_lossy()).await?;
    altevra_db::run_migrations(&pool).await?;
    let rows = sqlx::query(
        "SELECT id, packet_id, sensitivity_ceiling, created_at FROM exposure_decisions \
         ORDER BY created_at DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(&pool)
    .await?;
    if rows.is_empty() {
        println!("No exposure decisions recorded yet.");
        return Ok(());
    }
    println!("Exposure decisions ({}):", rows.len());
    for r in &rows {
        let id: String = r.get("id");
        let pkt: String = r.get::<Option<String>, _>("packet_id").unwrap_or_default();
        let ceil: String = r.get("sensitivity_ceiling");
        let at: String = r.get("created_at");
        println!("  {at}  ceiling={ceil}  packet={pkt}  id={id}");
    }
    Ok(())
}

//! `altevra brief` (P4) — render the proactive daily brief.
//!
//! Default: print the policy-GATED render (what the vault sees — relationship
//! items appear only as a withheld count + pointer). `--private` prints the
//! FULL version including policy-blocked personal signals — TERMINAL ONLY,
//! never written anywhere. `--write` also writes the gated render to
//! `<vault>/Daily/YYYY-MM-DD-altevra-brief.md`.
//!
//! Viewing takes NO dedup claims (a `brief` in the terminal must never eat a
//! notification window); only the brain's once-a-day delivery pass claims.

use altevra_brain::notify;
use clap::Args;
use std::path::PathBuf;

#[derive(Args)]
pub struct BriefArgs {
    /// Print the FULL private version (includes relationship/personal signals
    /// withheld from the vault by domain policy). Terminal only.
    #[arg(long)]
    pub private: bool,

    /// Also write the policy-gated brief to <vault>/Daily/YYYY-MM-DD-altevra-brief.md.
    #[arg(long)]
    pub write: bool,

    /// SQLite database path.
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,

    /// Vault root (default: config.toml [vault].path, then $ALTEVRA_VAULT).
    #[arg(long)]
    pub vault: Option<PathBuf>,
}

pub async fn run(args: BriefArgs) -> anyhow::Result<()> {
    let pool = altevra_db::create_pool(&args.db.to_string_lossy()).await?;
    altevra_db::run_migrations(&pool).await?;

    let vault = args
        .vault
        .unwrap_or_else(altevra_core::default_vault_path);
    let now = chrono::Utc::now();

    // View pass: claim = false — read-only, nothing suppressed, no window eaten.
    let items = notify::sources::collect_all(&pool, &vault, now).await;
    let cfg = notify::DeliveryConfig {
        claims_dir: notify::delivery::default_claims_dir(),
        claim: false,
    };
    let delivery = notify::deliver(&pool, &cfg, items, now).await?;
    let gate = altevra_brain::load_relevance_gate(&pool).await;
    let data = notify::build_brief_data(&pool, &delivery, &gate, now).await;

    // Terminal render: full when --private, gated otherwise.
    print!("{}", notify::render_brief(&data, args.private));

    if args.write {
        match notify::write_vault_brief(
            &pool,
            &vault,
            &notify::delivery::default_claims_dir(),
            false,
            &gate,
            now,
        )
        .await?
        {
            Some(p) => eprintln!("\nWritten: {}", p.display()),
            None => eprintln!(
                "\nBrief already exists: {}",
                vault
                    .join("Daily")
                    .join(format!("{}-altevra-brief.md", now.format("%Y-%m-%d")))
                    .display()
            ),
        }
    }
    Ok(())
}

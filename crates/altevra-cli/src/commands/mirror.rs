//! `altevra mirror plan` — DRY-RUN preview for the Imperium mirror writer
//! (P0 task E2, §2.14 D4).
//!
//! This is the human-facing seam for [`altevra_vault::write_mirror`]. It ONLY
//! offers a `plan` verb: it computes (target path, sha256, would-write bytes)
//! and surfaces the writer's policy decision (Planned / Skipped / Refused).
//! **NO `apply` verb is exposed here.** The writer's `dry_run = false` path is
//! reachable only by a future presence-gated command (require_human_presence)
//! once Pavle explicitly opts in — see the writer's module docs.
//!
//! The default `--root` is intentionally a `./mirror-plan` sandbox under cwd,
//! NOT `~/Obsidian/Imperium`. Pavle can point `--root` anywhere, but this
//! command never writes regardless.

use std::path::PathBuf;

use altevra_core::domain::Domain;
use altevra_core::envelope::{Envelope, Provenance, ProvenanceOrigin};
use altevra_core::security::Sensitivity;
use altevra_vault::{plan_mirror, WriteOutcome};
use chrono::Utc;
use clap::{Args, Subcommand};

#[derive(Subcommand)]
pub enum MirrorCommands {
    /// Compute a DRY-RUN plan for mirroring an object into the Imperium vault.
    /// Touches NOTHING on disk. Honors D4 (high-water / Confidential+ are
    /// always Skipped) and refuses to clobber human-edited targets.
    Plan(MirrorPlanArgs),
}

#[derive(Args)]
pub struct MirrorPlanArgs {
    /// Vault root to plan AGAINST. Defaults to a sandbox `./mirror-plan` under
    /// cwd — NEVER auto-targets `~/Obsidian/Imperium`. This command never
    /// writes, but the default keeps even the planned path off real vaults.
    #[arg(long)]
    pub root: Option<PathBuf>,
    /// Object id (envelope.id) — also drives the relative filename.
    #[arg(long)]
    pub id: String,
    /// Object type discriminator (e.g. `decision`, `learning`, `wiki_page`).
    #[arg(long = "type", default_value = "decision")]
    pub object_type: String,
    /// Primary domain (e.g. `business`, `public`). High-water domains
    /// (`personal`/`health`/`relationship`/`financial`/`legal`/`client`) are
    /// Skipped by policy — see D4.
    #[arg(long, default_value = "business")]
    pub domain: String,
    /// Sensitivity tier (`public`/`internal`/`restricted`/`confidential`/`secret`).
    /// `confidential` and above are Skipped by policy.
    #[arg(long, default_value = "internal")]
    pub sensitivity: String,
    /// Title rendered at the top of the mirror.
    #[arg(long)]
    pub title: String,
    /// Body — pass `-` to read from stdin.
    #[arg(long)]
    pub body: String,
    /// Emit JSON instead of human-readable text.
    #[arg(long)]
    pub json: bool,
}

pub async fn run(cmd: MirrorCommands) -> anyhow::Result<()> {
    match cmd {
        MirrorCommands::Plan(args) => plan(args).await,
    }
}

async fn plan(args: MirrorPlanArgs) -> anyhow::Result<()> {
    let root = args
        .root
        .clone()
        .unwrap_or_else(|| PathBuf::from("./mirror-plan"));
    let domain = parse_domain(&args.domain)?;
    let sensitivity = parse_sensitivity(&args.sensitivity)?;

    let body = if args.body == "-" {
        use std::io::Read;
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        s
    } else {
        args.body.clone()
    };

    let mut env = Envelope::new(
        &args.id,
        &args.object_type,
        Utc::now(),
        Provenance::new(ProvenanceOrigin::PavleDirect),
    );
    env.domain = domain;
    env.sensitivity = sensitivity;

    let outcome = plan_mirror(&root, &env, &args.title, &body)?;

    if args.json {
        println!("{}", outcome_to_json(&outcome));
    } else {
        print_outcome(&root, &outcome);
    }
    Ok(())
}

fn print_outcome(root: &std::path::Path, outcome: &WriteOutcome) {
    println!("altevra mirror plan — DRY RUN (nothing written)");
    println!("  root: {}", root.display());
    match outcome {
        WriteOutcome::Planned {
            target,
            relative_path,
            sha256,
            bytes,
        } => {
            println!("  decision: PLANNED");
            println!("  target: {}", target.display());
            println!("  relative_path: {relative_path}");
            println!("  sha256: {sha256}");
            println!("  would-write bytes: {bytes}");
            println!(
                "  note: this command never writes. Live writes require a future\n        presence-gated command (require_human_presence + explicit opt-in)."
            );
        }
        WriteOutcome::Skipped { reason } => {
            println!("  decision: SKIPPED");
            println!("  reason: {} (D4 — never mirrors)", reason.as_str());
        }
        WriteOutcome::Refused { target, reason } => {
            println!("  decision: REFUSED");
            println!("  target: {}", target.display());
            println!(
                "  reason: {} — Altevra refuses to clobber human edits.",
                reason.as_str()
            );
        }
        WriteOutcome::Wrote { .. } => {
            // Unreachable under plan_mirror, defensive print.
            println!("  decision: WROTE (unexpected for plan)");
        }
    }
}

fn outcome_to_json(outcome: &WriteOutcome) -> String {
    use serde_json::json;
    let v = match outcome {
        WriteOutcome::Planned {
            target,
            relative_path,
            sha256,
            bytes,
        } => json!({
            "outcome": "planned",
            "target": target.display().to_string(),
            "relative_path": relative_path,
            "sha256": sha256,
            "bytes": bytes,
            "wrote": false,
        }),
        WriteOutcome::Skipped { reason } => json!({
            "outcome": "skipped",
            "reason": reason.as_str(),
            "wrote": false,
        }),
        WriteOutcome::Refused { target, reason } => json!({
            "outcome": "refused",
            "target": target.display().to_string(),
            "reason": reason.as_str(),
            "wrote": false,
        }),
        WriteOutcome::Wrote {
            target,
            relative_path,
            sha256,
            bytes,
        } => json!({
            "outcome": "wrote",
            "target": target.display().to_string(),
            "relative_path": relative_path,
            "sha256": sha256,
            "bytes": bytes,
            "wrote": true,
        }),
    };
    serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".to_string())
}

fn parse_domain(s: &str) -> anyhow::Result<Domain> {
    // The CLI accepts only the 9 governed builtins (R1). `Other(...)` is not
    // user-pickable from the command line — it only exists for tolerance on
    // read of legacy storage.
    Ok(match s.to_ascii_lowercase().as_str() {
        "business" => Domain::Business,
        "personal" => Domain::Personal,
        "project" => Domain::Project,
        "client" => Domain::Client,
        "relationship" => Domain::Relationship,
        "health" => Domain::Health,
        "legal" => Domain::Legal,
        "financial" => Domain::Financial,
        "public" => Domain::Public,
        other => anyhow::bail!(
            "unknown domain: {other} \
             (business|personal|project|client|relationship|health|legal|financial|public)"
        ),
    })
}

fn parse_sensitivity(s: &str) -> anyhow::Result<Sensitivity> {
    Ok(match s.to_ascii_lowercase().as_str() {
        "public" => Sensitivity::Public,
        "internal" => Sensitivity::Internal,
        "restricted" => Sensitivity::Restricted,
        "confidential" => Sensitivity::Confidential,
        "secret" => Sensitivity::Secret,
        other => anyhow::bail!(
            "unknown sensitivity: {other} (public|internal|restricted|confidential|secret)"
        ),
    })
}

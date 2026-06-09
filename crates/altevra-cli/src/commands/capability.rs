//! `altevra capability` — adapter-dossier + capability-record surface
//! (PLAN-ALIVE §P1.4).
//!
//! * `seed` — load `adapter_dossiers` from
//!   `~/.imperium/capabilities/{claude,hermes}.yaml` (graceful skip when a
//!   file is absent). Fields are guarded at upsert in altevra-db (§P1.3).
//! * `list` — dossiers + capability records (optionally per actor).
//! * `record` — write an honest can/cannot/unverified capability record
//!   (T7: `supported` REQUIRES an evidence ref — enforced by the repo).

use altevra_db::{
    create_pool, run_migrations, AdapterDossierRow, AdapterDossiersRepository,
    CapabilityRecordRow, CapabilityRecordsRepository,
};
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum CapabilityCommands {
    /// Load adapter dossiers from ~/.imperium/capabilities/{claude,hermes}.yaml.
    Seed(CapabilitySeedArgs),
    /// List adapter dossiers and capability records.
    List(CapabilityListArgs),
    /// Record an honest capability for an actor.
    Record(CapabilityRecordArgs),
}

#[derive(Args)]
pub struct CapabilitySeedArgs {
    /// Directory holding the agent capability YAMLs.
    #[arg(long)]
    pub dir: Option<PathBuf>,

    #[arg(long)]
    pub dry_run: bool,

    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
}

#[derive(Args)]
pub struct CapabilityListArgs {
    /// Filter capability records by actor.
    #[arg(long)]
    pub actor: Option<String>,

    #[arg(long)]
    pub json: bool,

    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
}

#[derive(Args)]
pub struct CapabilityRecordArgs {
    /// Actor (altevra|claude-code|codex|cursor|hermes|...).
    pub actor: String,

    /// Capability key (e.g. hook.session_start, mcp.tools).
    pub key: String,

    /// supported|unsupported|unverified|fallback.
    #[arg(long, default_value = "unverified")]
    pub support: String,

    /// REQUIRED when --support supported (T7 honesty).
    #[arg(long)]
    pub evidence_ref: Option<String>,

    /// tested|declared|observed.
    #[arg(long)]
    pub method: Option<String>,

    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
}

pub async fn run(cmd: CapabilityCommands) -> anyhow::Result<()> {
    match cmd {
        CapabilityCommands::Seed(args) => run_seed(args).await,
        CapabilityCommands::List(args) => run_list(args).await,
        CapabilityCommands::Record(args) => run_record(args).await,
    }
}

/// Parse one agent capability YAML into a dossier row. Returns None when the
/// file is missing or carries no `agent:` key.
pub fn dossier_from_yaml(path: &std::path::Path) -> Option<AdapterDossierRow> {
    let body = std::fs::read_to_string(path).ok()?;
    let yaml: serde_yaml::Value = serde_yaml::from_str(&body).ok()?;
    let agent = yaml.get("agent")?.as_str()?;
    let tool_name = crate::commands::tool::map_agent_name(agent);

    let str_list = |key: &str| -> Vec<String> {
        yaml.get(key)
            .and_then(|v| v.as_sequence())
            .map(|seq| {
                seq.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };
    let can = str_list("can");
    let cannot = str_list("cannot");
    // hooks.* entries in `can` describe supported hook events.
    let hook_events: Vec<String> = can
        .iter()
        .filter_map(|c| c.strip_prefix("hooks.").map(String::from))
        .collect();
    let version = yaml
        .get("version")
        .map(|v| match v {
            serde_yaml::Value::String(s) => s.clone(),
            other => serde_yaml::to_string(other).unwrap_or_default().trim().to_string(),
        })
        .unwrap_or_else(|| "unknown".to_string());

    Some(AdapterDossierRow {
        id: uuid::Uuid::new_v4().to_string(),
        tool_name,
        adapter_version: version,
        // Snapshot YAMLs are DECLARED, not tested — honesty keeps the tier
        // at `unverified` until a verify run upgrades it.
        support_tier: "unverified".to_string(),
        surfaces: serde_json::json!({"can": can, "cannot": cannot}),
        hook_events_supported: serde_json::json!(hook_events),
        skill_format: None,
        detection: yaml
            .get("binary")
            .and_then(|v| v.as_str())
            .map(String::from),
    })
}

async fn run_seed(args: CapabilitySeedArgs) -> anyhow::Result<()> {
    if !args.dry_run && crate::commands::brain::refuse_if_maintenance_locked("capability seed") {
        return Ok(());
    }
    let dir = args
        .dir
        .unwrap_or_else(|| altevra_core::home_dir().join(".imperium/capabilities"));

    let mut dossiers = Vec::new();
    for file in ["claude.yaml", "hermes.yaml"] {
        let p = dir.join(file);
        match dossier_from_yaml(&p) {
            Some(d) => dossiers.push(d),
            None => println!("  skip {} (absent or not an agent snapshot)", p.display()),
        }
    }
    if dossiers.is_empty() {
        println!("No capability YAMLs found under {} — nothing seeded.", dir.display());
        return Ok(());
    }
    if args.dry_run {
        println!("DRY-RUN — {} dossier(s) would be upserted:", dossiers.len());
        for d in &dossiers {
            println!(
                "  {} v{} ({} can / {} hook events)",
                d.tool_name,
                d.adapter_version,
                d.surfaces["can"].as_array().map(|a| a.len()).unwrap_or(0),
                d.hook_events_supported.as_array().map(|a| a.len()).unwrap_or(0),
            );
        }
        return Ok(());
    }

    let pool = create_pool(&args.db.to_string_lossy()).await?;
    run_migrations(&pool).await?;
    let repo = AdapterDossiersRepository::new(&pool);
    let mut sightings = 0usize;
    for d in &dossiers {
        sightings += repo.upsert(d).await?;
        println!("  seeded adapter dossier: {}", d.tool_name);
    }
    if sightings > 0 {
        println!("  ({sightings} secret sighting(s) redacted + logged)");
    }
    Ok(())
}

async fn run_list(args: CapabilityListArgs) -> anyhow::Result<()> {
    let pool = create_pool(&args.db.to_string_lossy()).await?;
    run_migrations(&pool).await?;
    let dossiers = AdapterDossiersRepository::new(&pool).list().await?;
    let records = CapabilityRecordsRepository::new(&pool)
        .list(args.actor.as_deref())
        .await?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "adapter_dossiers": dossiers.iter().map(|d| serde_json::json!({
                    "tool_name": d.tool_name,
                    "adapter_version": d.adapter_version,
                    "support_tier": d.support_tier,
                    "surfaces": d.surfaces,
                    "hook_events_supported": d.hook_events_supported,
                    "detection": d.detection,
                })).collect::<Vec<_>>(),
                "capability_records": records.iter().map(|r| serde_json::json!({
                    "actor": r.actor,
                    "capability_key": r.capability_key,
                    "support": r.support,
                    "evidence_ref": r.evidence_ref,
                })).collect::<Vec<_>>(),
            }))?
        );
        return Ok(());
    }

    println!("{} adapter dossier(s):", dossiers.len());
    for d in &dossiers {
        println!(
            "  {:14} v{:10} tier={}",
            d.tool_name, d.adapter_version, d.support_tier
        );
    }
    println!("{} capability record(s):", records.len());
    for r in &records {
        println!("  {:14} {:30} {}", r.actor, r.capability_key, r.support);
    }
    Ok(())
}

async fn run_record(args: CapabilityRecordArgs) -> anyhow::Result<()> {
    if crate::commands::brain::refuse_if_maintenance_locked("capability record") {
        return Ok(());
    }
    let pool = create_pool(&args.db.to_string_lossy()).await?;
    run_migrations(&pool).await?;
    let row = CapabilityRecordRow {
        id: uuid::Uuid::new_v4().to_string(),
        actor: args.actor.clone(),
        capability_key: args.key.clone(),
        support: args.support.clone(),
        evidence_ref: args.evidence_ref.clone(),
        verification_method: args.method.clone(),
    };
    // T7 honesty (supported requires evidence) is enforced by the repository.
    CapabilityRecordsRepository::new(&pool).upsert(&row).await?;
    println!("Recorded {}:{} = {}", args.actor, args.key, args.support);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn dossier_from_yaml_maps_fields_and_handles_missing() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("claude.yaml");
        std::fs::write(
            &p,
            "agent: claude\nversion: 2.1.150\nbinary: /x/bin/claude\n\
             can:\n  - code.read\n  - hooks.SessionStart\n  - hooks.PostToolUse\n\
             cannot:\n  - cron.create\n",
        )
        .unwrap();
        let d = dossier_from_yaml(&p).unwrap();
        assert_eq!(d.tool_name, "claude-code", "agent 'claude' maps to claude-code");
        assert_eq!(d.adapter_version, "2.1.150");
        assert_eq!(d.support_tier, "unverified", "declared YAML never claims verified");
        assert_eq!(
            d.hook_events_supported,
            serde_json::json!(["SessionStart", "PostToolUse"])
        );
        assert_eq!(d.surfaces["cannot"], serde_json::json!(["cron.create"]));
        assert_eq!(d.detection.as_deref(), Some("/x/bin/claude"));

        // Missing file / non-agent yaml → graceful None.
        assert!(dossier_from_yaml(&tmp.path().join("absent.yaml")).is_none());
        let m = tmp.path().join("manifest.yaml");
        std::fs::write(&m, "tools:\n  - x\n").unwrap();
        assert!(dossier_from_yaml(&m).is_none());
    }

    #[tokio::test]
    async fn seed_upserts_idempotently_into_temp_db() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("hermes.yaml");
        std::fs::write(
            &p,
            "agent: hermes\nversion: 0.14.0\nbinary: /x/bin/hermes\ncan:\n  - cron.create\n",
        )
        .unwrap();
        let db = tmp.path().join("altevra.db");
        let pool = create_pool(&db.to_string_lossy()).await.unwrap();
        run_migrations(&pool).await.unwrap();
        let repo = AdapterDossiersRepository::new(&pool);

        let d = dossier_from_yaml(&p).unwrap();
        repo.upsert(&d).await.unwrap();
        let d2 = dossier_from_yaml(&p).unwrap();
        repo.upsert(&d2).await.unwrap();

        let all = repo.list().await.unwrap();
        assert_eq!(all.len(), 1, "tool_name upsert must merge, not duplicate");
        assert_eq!(all[0].tool_name, "hermes");
        assert_eq!(all[0].surfaces["can"], serde_json::json!(["cron.create"]));
    }
}

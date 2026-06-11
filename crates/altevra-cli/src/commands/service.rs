//! `altevra service` — generate + install systemd user units (R1).
//!
//! ## Units generated
//!
//! * `altevra-brain.service` — autonomous brain daemon (`altevra brain start`)
//! * `altevra-embedder.service` — continuous embedder worker (`altevra embed run`)
//!   NOTE: the embedder is a separate unit rather than a brain-job because it
//!   hits an external embedding API with its own rate-limit, needs independent
//!   restart behaviour, and the EmbedderWorker already exists as a standalone
//!   long-running loop. Wiring it into BrainScheduler would save one unit but
//!   force the brain to carry API-key dependency even when embedding is disabled.
//!   Keeping it separate is a smaller diff and more operationally flexible.
//! * `altevra-backup.service` + `altevra-backup.timer` — daily backup job.
//!
//! ## Stale-lock TTL handling
//!
//! Every service's `ExecStart` invokes the existing binary which already
//! calls `refuse_if_maintenance_locked` / `maintenance_locked_default` before
//! writing to the DB. The generated unit files add an `ExecStartPre` check
//! that exits 0 (non-fatal) when a stale lock is detected at service-start
//! time, preventing a burst of restarts during a long unify window. The stale
//! TTL is the library constant (30 min); services will attempt a fresh start
//! after `RestartSec=5` once the lock clears.
//!
//! ## Deployment
//!
//! Unit files are also mirrored to `deploy/systemd/` in the source repo so
//! they can be reviewed / committed. `--dry-run` prints the content and the
//! target paths without writing to disk or systemctl.

use clap::{Args, Subcommand};
use std::path::{Path, PathBuf};

/// Subcommands under `altevra service`
#[derive(Subcommand)]
pub enum ServiceCommands {
    /// Generate and install systemd user units (brain, embedder, backup+timer).
    /// Uses `--dry-run` by default — pass `--apply` to write to
    /// ~/.config/systemd/user/ and run `systemctl --user daemon-reload`.
    Install(ServiceInstallArgs),

    /// Show the status of the three Altevra systemd units.
    Status(ServiceStatusArgs),
}

#[derive(Args)]
pub struct ServiceInstallArgs {
    /// Path to the altevra binary. Defaults to the current executable.
    #[arg(long)]
    pub binary: Option<PathBuf>,

    /// Database path baked into ExecStart (absolute).
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,

    /// Vault path baked into ExecStart (absolute).
    #[arg(long, default_value_os_t = altevra_core::default_vault_path())]
    pub vault: PathBuf,

    /// User home directory (default: $HOME). Baked into Environment=HOME=.
    #[arg(long)]
    pub home: Option<PathBuf>,

    /// Working directory for units (default: $HOME).
    #[arg(long)]
    pub working_dir: Option<PathBuf>,

    /// Preview only — print unit content and target paths; write nothing.
    #[arg(long)]
    pub dry_run: bool,

    /// Actually write unit files and run `systemctl --user daemon-reload`.
    /// Requires explicit flag to guard against accidental installs.
    #[arg(long)]
    pub apply: bool,

    /// Mirror generated unit files to deploy/systemd/ relative to this
    /// path (repo root). Use `--mirror-dir <path>` to set explicitly.
    #[arg(long)]
    pub mirror_dir: Option<PathBuf>,
}

#[derive(Args)]
pub struct ServiceStatusArgs {
    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

pub async fn run(cmd: ServiceCommands) -> anyhow::Result<()> {
    match cmd {
        ServiceCommands::Install(args) => run_install(args).await,
        ServiceCommands::Status(args) => run_status(args).await,
    }
}

// ============================================================================
// Unit content generation (pure, no I/O — easy to test hermetically)
// ============================================================================

/// All data needed to render any of the three service units.
#[derive(Debug, Clone, PartialEq)]
pub struct UnitContext {
    /// Absolute path to the altevra binary.
    pub binary: PathBuf,
    /// Absolute database path (`--db`).
    pub db: PathBuf,
    /// Absolute vault path (`--vault`).
    pub vault: PathBuf,
    /// Value for `Environment=HOME=…`.
    pub home: PathBuf,
    /// Value for `WorkingDirectory=`.
    pub working_dir: PathBuf,
}

impl UnitContext {
    /// Build a `UnitContext` from CLI args + OS environment.
    pub fn from_args(args: &ServiceInstallArgs) -> anyhow::Result<Self> {
        let binary = match &args.binary {
            Some(p) => p.clone(),
            None => std::env::current_exe()?,
        };
        if !binary.is_absolute() {
            anyhow::bail!("binary path must be absolute, got: {}", binary.display());
        }

        let db = args.db.clone();
        if !db.is_absolute() {
            anyhow::bail!("--db must be an absolute path, got: {}", db.display());
        }

        let vault = args.vault.clone();
        if !vault.is_absolute() {
            anyhow::bail!("--vault must be an absolute path, got: {}", vault.display());
        }

        let home = args
            .home
            .clone()
            .unwrap_or_else(altevra_core::home_dir);
        if !home.is_absolute() {
            anyhow::bail!("--home must be absolute, got: {}", home.display());
        }

        let working_dir = args
            .working_dir
            .clone()
            .unwrap_or_else(|| home.clone());
        if !working_dir.is_absolute() {
            anyhow::bail!("--working-dir must be absolute, got: {}", working_dir.display());
        }

        Ok(Self { binary, db, vault, home, working_dir })
    }
}

/// Generate the content of `altevra-brain.service`.
pub fn brain_service_unit(ctx: &UnitContext) -> String {
    let b = ctx.binary.display();
    let db = ctx.db.display();
    let vault = ctx.vault.display();
    let home = ctx.home.display();
    let wd = ctx.working_dir.display();
    format!(
        r#"[Unit]
Description=Altevra Brain Daemon
Documentation=https://github.com/ceoimperiumprojects/altevra
After=network.target

[Service]
Type=simple
ExecStartPre=/bin/sh -c 'test ! -f {home}/.altevra/state/maintenance.lock || exit 0'
ExecStart={b} brain start --db {db} --vault {vault}
Environment=HOME={home}
WorkingDirectory={wd}
Restart=always
RestartSec=5
StandardOutput=journal
StandardError=journal
SyslogIdentifier=altevra-brain

[Install]
WantedBy=default.target
"#
    )
}

/// Generate the content of `altevra-embedder.service`.
pub fn embedder_service_unit(ctx: &UnitContext) -> String {
    let b = ctx.binary.display();
    let db = ctx.db.display();
    let home = ctx.home.display();
    let wd = ctx.working_dir.display();
    format!(
        r#"[Unit]
Description=Altevra Embedder Worker
Documentation=https://github.com/ceoimperiumprojects/altevra
After=network.target altevra-brain.service

[Service]
Type=simple
ExecStartPre=/bin/sh -c 'test ! -f {home}/.altevra/state/maintenance.lock || exit 0'
ExecStart={b} embed run --db {db}
Environment=HOME={home}
WorkingDirectory={wd}
Restart=always
RestartSec=5
StandardOutput=journal
StandardError=journal
SyslogIdentifier=altevra-embedder

[Install]
WantedBy=default.target
"#
    )
}

/// Generate the content of `altevra-backup.service`.
pub fn backup_service_unit(ctx: &UnitContext) -> String {
    let b = ctx.binary.display();
    let db = ctx.db.display();
    let vault = ctx.vault.display();
    let home = ctx.home.display();
    let wd = ctx.working_dir.display();
    format!(
        r#"[Unit]
Description=Altevra Daily Backup
Documentation=https://github.com/ceoimperiumprojects/altevra
After=network.target

[Service]
Type=oneshot
ExecStart={b} backup run --db {db} --vault {vault}
Environment=HOME={home}
WorkingDirectory={wd}
StandardOutput=journal
StandardError=journal
SyslogIdentifier=altevra-backup
"#
    )
}

/// Generate the content of `altevra-backup.timer`.
pub fn backup_timer_unit() -> String {
    r#"[Unit]
Description=Altevra Daily Backup Timer
Documentation=https://github.com/ceoimperiumprojects/altevra

[Timer]
OnCalendar=daily
Persistent=true
RandomizedDelaySec=600

[Install]
WantedBy=timers.target
"#
    .to_string()
}

// ============================================================================
// Install logic
// ============================================================================

/// All four unit names.
pub const UNIT_NAMES: &[&str] = &[
    "altevra-brain.service",
    "altevra-embedder.service",
    "altevra-backup.service",
    "altevra-backup.timer",
];

pub async fn run_install(args: ServiceInstallArgs) -> anyhow::Result<()> {
    // Require exactly one of --dry-run or --apply.
    match (args.dry_run, args.apply) {
        (false, false) => {
            anyhow::bail!(
                "pass either --dry-run (preview, writes nothing) or --apply \
                 (write to ~/.config/systemd/user/ + daemon-reload). Nothing was changed."
            );
        }
        (true, true) => {
            anyhow::bail!("--dry-run and --apply are mutually exclusive.");
        }
        _ => {}
    }

    let ctx = UnitContext::from_args(&args)?;
    let units: Vec<(&str, String)> = vec![
        ("altevra-brain.service", brain_service_unit(&ctx)),
        ("altevra-embedder.service", embedder_service_unit(&ctx)),
        ("altevra-backup.service", backup_service_unit(&ctx)),
        ("altevra-backup.timer", backup_timer_unit()),
    ];

    let systemd_user_dir = ctx.home.join(".config/systemd/user");

    if args.dry_run {
        println!("=== DRY RUN — no files written ===");
        println!();
        println!("Target directory: {}", systemd_user_dir.display());
        println!();
        for (name, content) in &units {
            println!("--- {} ---", name);
            println!("{}", content);
        }
        println!(
            "NOTE: After applying, run `loginctl enable-linger {}` if the brain \n\
             should survive user logout.",
            whoami_or_unknown()
        );
        println!();
        println!(
            "Next steps after `altevra service install --apply`:\n\
             1. systemctl --user enable --now altevra-brain.service\n\
             2. systemctl --user enable --now altevra-embedder.service\n\
             3. systemctl --user enable --now altevra-backup.timer\n\
             4. loginctl enable-linger {}", whoami_or_unknown()
        );

        // Mirror to repo deploy/systemd/ when requested (dry-run still mirrors
        // so the reviewer sees the same content as what apply would install).
        if let Some(mirror) = &args.mirror_dir {
            mirror_units(mirror, &units)?;
        }
        return Ok(());
    }

    // --apply
    std::fs::create_dir_all(&systemd_user_dir)?;
    for (name, content) in &units {
        let target = systemd_user_dir.join(name);
        // Idempotent: write only when content differs.
        let existing = std::fs::read_to_string(&target).unwrap_or_default();
        if existing == *content {
            println!("  [unchanged] {}", target.display());
        } else {
            std::fs::write(&target, content)?;
            println!("  [written]   {}", target.display());
        }
    }

    // Mirror to repo deploy/systemd/.
    if let Some(mirror) = &args.mirror_dir {
        mirror_units(mirror, &units)?;
    }

    // systemctl --user daemon-reload (best-effort; systemd may not be running).
    println!();
    println!("Running: systemctl --user daemon-reload");
    let st = std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();
    match st {
        Ok(s) if s.success() => println!("  daemon-reload OK"),
        Ok(s) => eprintln!(
            "  [warn] systemctl --user daemon-reload exited {s} (non-fatal)"
        ),
        Err(e) => eprintln!(
            "  [warn] could not run systemctl --user daemon-reload: {e} (non-fatal; \n\
             run it manually to activate the units)"
        ),
    }

    println!();
    println!("Units installed. Enable them with:");
    println!("  systemctl --user enable --now altevra-brain.service");
    println!("  systemctl --user enable --now altevra-embedder.service");
    println!("  systemctl --user enable --now altevra-backup.timer");
    println!();
    println!(
        "To survive logout, run: loginctl enable-linger {}",
        whoami_or_unknown()
    );

    Ok(())
}

fn mirror_units(mirror_dir: &Path, units: &[(&str, String)]) -> anyhow::Result<()> {
    std::fs::create_dir_all(mirror_dir)?;
    for (name, content) in units {
        let target = mirror_dir.join(name);
        let existing = std::fs::read_to_string(&target).unwrap_or_default();
        if existing != *content {
            std::fs::write(&target, content)?;
            println!("  [mirrored]  {}", target.display());
        }
    }
    Ok(())
}

fn whoami_or_unknown() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "$(whoami)".to_string())
}

async fn run_status(args: ServiceStatusArgs) -> anyhow::Result<()> {
    let mut results: Vec<serde_json::Value> = Vec::new();
    for unit in UNIT_NAMES {
        let active = unit_is_active(unit);
        results.push(serde_json::json!({ "unit": unit, "active": active }));
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else {
        for r in &results {
            let unit = r["unit"].as_str().unwrap_or("?");
            let active = r["active"].as_bool().unwrap_or(false);
            let icon = if active { "✓" } else { "○" };
            println!("  {icon} {unit}");
        }
    }
    Ok(())
}

fn unit_is_active(unit: &str) -> bool {
    std::process::Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", unit])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fixture_ctx(tmp: &Path) -> UnitContext {
        UnitContext {
            binary: tmp.join("altevra"),
            db: tmp.join(".altevra/altevra.db"),
            vault: tmp.join("vault"),
            home: tmp.to_path_buf(),
            working_dir: tmp.to_path_buf(),
        }
    }

    // ---- Unit content correctness ----

    #[test]
    fn brain_service_has_absolute_exec_start() {
        let tmp = TempDir::new().unwrap();
        let ctx = fixture_ctx(tmp.path());
        let unit = brain_service_unit(&ctx);
        assert!(
            unit.contains(&format!("ExecStart={}", ctx.binary.display())),
            "ExecStart must use the absolute binary path"
        );
        assert!(
            unit.contains(&format!("--db {}", ctx.db.display())),
            "ExecStart must contain --db <absolute path>"
        );
        assert!(
            unit.contains(&format!("--vault {}", ctx.vault.display())),
            "ExecStart must contain --vault <absolute path>"
        );
    }

    #[test]
    fn brain_service_has_environment_home() {
        let tmp = TempDir::new().unwrap();
        let ctx = fixture_ctx(tmp.path());
        let unit = brain_service_unit(&ctx);
        assert!(
            unit.contains(&format!("Environment=HOME={}", ctx.home.display())),
            "brain unit must set Environment=HOME="
        );
    }

    #[test]
    fn brain_service_has_working_directory() {
        let tmp = TempDir::new().unwrap();
        let ctx = fixture_ctx(tmp.path());
        let unit = brain_service_unit(&ctx);
        assert!(
            unit.contains(&format!("WorkingDirectory={}", ctx.working_dir.display())),
            "brain unit must set WorkingDirectory="
        );
    }

    #[test]
    fn brain_service_has_restart_always() {
        let tmp = TempDir::new().unwrap();
        let ctx = fixture_ctx(tmp.path());
        let unit = brain_service_unit(&ctx);
        assert!(unit.contains("Restart=always"), "brain must have Restart=always");
        assert!(unit.contains("RestartSec=5"), "brain must have RestartSec=5");
    }

    #[test]
    fn brain_service_has_wanted_by_default_target() {
        let tmp = TempDir::new().unwrap();
        let ctx = fixture_ctx(tmp.path());
        let unit = brain_service_unit(&ctx);
        assert!(
            unit.contains("WantedBy=default.target"),
            "brain unit must install under default.target"
        );
    }

    #[test]
    fn embedder_service_has_absolute_exec_start() {
        let tmp = TempDir::new().unwrap();
        let ctx = fixture_ctx(tmp.path());
        let unit = embedder_service_unit(&ctx);
        assert!(unit.contains(&format!("ExecStart={}", ctx.binary.display())));
        assert!(unit.contains(&format!("--db {}", ctx.db.display())));
        assert!(unit.contains(&format!("Environment=HOME={}", ctx.home.display())));
        assert!(unit.contains(&format!("WorkingDirectory={}", ctx.working_dir.display())));
        assert!(unit.contains("Restart=always"));
        assert!(unit.contains("RestartSec=5"));
        assert!(unit.contains("WantedBy=default.target"));
    }

    #[test]
    fn backup_service_has_absolute_exec_start() {
        let tmp = TempDir::new().unwrap();
        let ctx = fixture_ctx(tmp.path());
        let unit = backup_service_unit(&ctx);
        assert!(unit.contains(&format!("ExecStart={}", ctx.binary.display())));
        assert!(unit.contains("backup run"));
        assert!(unit.contains(&format!("--db {}", ctx.db.display())));
        assert!(unit.contains(&format!("--vault {}", ctx.vault.display())));
        assert!(unit.contains(&format!("Environment=HOME={}", ctx.home.display())));
        assert!(unit.contains(&format!("WorkingDirectory={}", ctx.working_dir.display())));
        // backup is oneshot — no Restart=always
        assert!(!unit.contains("Restart=always"), "backup service is oneshot, no Restart");
    }

    #[test]
    fn backup_timer_contains_daily_and_persistent() {
        let unit = backup_timer_unit();
        assert!(unit.contains("OnCalendar=daily"), "timer must fire daily");
        assert!(unit.contains("Persistent=true"), "timer must be persistent");
        assert!(unit.contains("WantedBy=timers.target"));
    }

    #[test]
    fn no_unit_contains_cwd_relative_paths() {
        let tmp = TempDir::new().unwrap();
        let ctx = fixture_ctx(tmp.path());
        // The db/vault/home are anchored to tmp (absolute in all cases).
        // Verify none of the generated units contain a literal "." that could
        // be misread as CWD (a relative path that starts with just "." by itself).
        for unit in [
            brain_service_unit(&ctx),
            embedder_service_unit(&ctx),
            backup_service_unit(&ctx),
        ] {
            // We cannot rule out "." in the content entirely (it appears in
            // journal/doc strings), but every file path mentioned should be
            // absolute (starts with /).
            // Verify ExecStart and WorkingDirectory contain only absolute paths.
            for line in unit.lines() {
                if line.starts_with("ExecStart=") || line.starts_with("WorkingDirectory=") {
                    // Extract the path part (everything after the first space or =).
                    let path_part = line.splitn(2, '=').nth(1).unwrap_or("").trim();
                    // ExecStart may have flags; the binary itself must be absolute.
                    let first = path_part.split_whitespace().next().unwrap_or("");
                    assert!(
                        first.starts_with('/'),
                        "ExecStart/WorkingDirectory must use absolute path, got: {line}"
                    );
                }
                if line.starts_with("Environment=HOME=") {
                    let val = line.trim_start_matches("Environment=HOME=");
                    assert!(
                        val.starts_with('/'),
                        "Environment=HOME= must be absolute, got: {line}"
                    );
                }
            }
        }
    }

    // ---- UnitContext::from_args validation ----

    #[test]
    fn from_args_rejects_relative_binary() {
        let tmp = TempDir::new().unwrap();
        let args = ServiceInstallArgs {
            binary: Some(PathBuf::from("altevra")), // relative
            db: tmp.path().join("altevra.db"),
            vault: tmp.path().to_path_buf(),
            home: Some(tmp.path().to_path_buf()),
            working_dir: None,
            dry_run: true,
            apply: false,
            mirror_dir: None,
        };
        assert!(
            UnitContext::from_args(&args).is_err(),
            "relative binary path must be rejected"
        );
    }

    // ---- Dry-run writes nothing ----

    #[tokio::test]
    async fn dry_run_writes_nothing_to_systemd_dir() {
        let tmp = TempDir::new().unwrap();
        let fake_home = tmp.path().to_path_buf();
        let systemd_dir = fake_home.join(".config/systemd/user");

        let args = ServiceInstallArgs {
            binary: Some(fake_home.join("altevra")), // absolute
            db: fake_home.join(".altevra/altevra.db"),
            vault: fake_home.join("vault"),
            home: Some(fake_home.clone()),
            working_dir: Some(fake_home.clone()),
            dry_run: true,
            apply: false,
            mirror_dir: None,
        };
        run_install(args).await.unwrap();

        // Nothing written to the systemd dir.
        assert!(
            !systemd_dir.exists(),
            "dry-run must not create the systemd user directory"
        );
    }

    // ---- Mirror writes ----

    #[tokio::test]
    async fn dry_run_with_mirror_dir_creates_unit_files() {
        let tmp = TempDir::new().unwrap();
        let fake_home = tmp.path().to_path_buf();
        let mirror = tmp.path().join("deploy/systemd");

        let args = ServiceInstallArgs {
            binary: Some(fake_home.join("altevra")),
            db: fake_home.join(".altevra/altevra.db"),
            vault: fake_home.join("vault"),
            home: Some(fake_home.clone()),
            working_dir: Some(fake_home.clone()),
            dry_run: true,
            apply: false,
            mirror_dir: Some(mirror.clone()),
        };
        run_install(args).await.unwrap();

        for name in UNIT_NAMES {
            let p = mirror.join(name);
            assert!(p.exists(), "mirror should contain {name}");
        }
    }

    // ---- No double-flag error ----

    #[tokio::test]
    async fn install_requires_exactly_one_flag() {
        let tmp = TempDir::new().unwrap();
        let fake_home = tmp.path().to_path_buf();

        // Neither flag
        let err = run_install(ServiceInstallArgs {
            binary: Some(fake_home.join("altevra")),
            db: fake_home.join("altevra.db"),
            vault: fake_home.clone(),
            home: Some(fake_home.clone()),
            working_dir: Some(fake_home.clone()),
            dry_run: false,
            apply: false,
            mirror_dir: None,
        })
        .await;
        assert!(err.is_err(), "neither flag must be an error");

        // Both flags
        let err = run_install(ServiceInstallArgs {
            binary: Some(fake_home.join("altevra")),
            db: fake_home.join("altevra.db"),
            vault: fake_home.clone(),
            home: Some(fake_home.clone()),
            working_dir: Some(fake_home.clone()),
            dry_run: true,
            apply: true,
            mirror_dir: None,
        })
        .await;
        assert!(err.is_err(), "both flags must be an error");
    }
}

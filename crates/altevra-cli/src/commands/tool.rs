//! `altevra tool` — the Tool Register surface (PLAN-ALIVE §P1).
//!
//! * `scan` — discover invocable tools across $PATH, npm-global, Claude
//!   skills, `~/.imperium/capabilities/*.yaml`, and source checkouts under
//!   `~/projekti`. **Reconciliation key is `(name, kind)`** — the same tool
//!   found in multiple places becomes ONE row with all locations in
//!   `locations[]`. Realpath is used ONLY to dedup identical-file PATH
//!   aliases (symlinks); version-manager shim dirs (mise/asdf/nvm) are
//!   denylisted because every shim realpaths to one binary.
//! * `seed` — upsert the 15-tool `SEED_TOOLS` baseline (ASSESSMENT §4).
//! * `list` / `register` / `verify` — query + manual curation.
//!
//! Security (§P1.3, mandatory): every persisted field passes the guard at
//! upsert inside `ToolRecordsRepository` (altevra-db); the source-checkout
//! scan additionally inherits the S3 DENY globs (`**/auth*`, `**/*token*`,
//! `**/*secret*`, `**/*.env*`, db files) and never opens those paths.

use altevra_db::{
    create_pool, run_migrations, AdapterDossiersRepository, ToolRecordRow, ToolRecordsRepository,
};
use clap::{Args, Subcommand};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub enum ToolCommands {
    /// Discover tools across scan sources and upsert into the register.
    Scan(ToolScanArgs),
    /// List registered tools.
    List(ToolListArgs),
    /// Manually register (or update) a tool.
    Register(ToolRegisterArgs),
    /// Record an honest verification status for a tool.
    Verify(ToolVerifyArgs),
    /// Upsert the 15-tool SEED_TOOLS baseline (idempotent).
    Seed(ToolSeedArgs),
}

#[derive(Args)]
pub struct ToolScanArgs {
    /// Print what would be upserted without writing the database.
    #[arg(long)]
    pub dry_run: bool,

    /// Source-checkout root to scan (shallow). Defaults to ~/projekti.
    #[arg(long)]
    pub checkouts: Option<PathBuf>,

    /// SQLite database path.
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
}

#[derive(Args)]
pub struct ToolListArgs {
    /// Filter by kind (skill|cli|python-api|mcp-server|web-service|adb|binary).
    #[arg(long)]
    pub kind: Option<String>,

    /// Filter by status (can|cannot|unverified).
    #[arg(long)]
    pub status: Option<String>,

    #[arg(long)]
    pub json: bool,

    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
}

#[derive(Args)]
pub struct ToolRegisterArgs {
    /// Tool name.
    pub name: String,

    /// Tool kind (skill|cli|python-api|mcp-server|web-service|adb|binary).
    #[arg(long)]
    pub kind: String,

    #[arg(long)]
    pub description: Option<String>,

    /// Canonical invocation, e.g. "imperium-crawl <cmd>".
    #[arg(long)]
    pub invocation: Option<String>,

    #[arg(long)]
    pub display_name: Option<String>,

    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
}

#[derive(Args)]
pub struct ToolVerifyArgs {
    /// Tool name.
    pub name: String,

    /// Tool kind the verification applies to.
    #[arg(long)]
    pub kind: String,

    /// Honest verification status.
    #[arg(long)]
    pub status: String,

    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
}

#[derive(Args)]
pub struct ToolSeedArgs {
    #[arg(long)]
    pub dry_run: bool,

    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
}

pub async fn run(cmd: ToolCommands) -> anyhow::Result<()> {
    match cmd {
        ToolCommands::Scan(args) => run_scan(args).await,
        ToolCommands::List(args) => run_list(args).await,
        ToolCommands::Register(args) => run_register(args).await,
        ToolCommands::Verify(args) => run_verify(args).await,
        ToolCommands::Seed(args) => run_seed(args).await,
    }
}

// ---------------------------------------------------------------------------
// SEED_TOOLS — 15 priority tools (ASSESSMENT.md §4 table). Discovery baseline;
// `altevra tool seed` upserts them idempotently.
// ---------------------------------------------------------------------------

pub struct SeedTool {
    pub name: &'static str,
    pub kind: &'static str,
    pub invocation: &'static str,
    pub description: &'static str,
    pub status: &'static str,
}

pub const SEED_TOOLS: [SeedTool; 15] = [
    SeedTool { name: "imperium-crawl", kind: "cli", invocation: "imperium-crawl <cmd>", description: "Browser automation / crawling CLI (also via browser-automation skill)", status: "can" },
    SeedTool { name: "chatgpt-py", kind: "cli", invocation: "chatgpt / /chatgpt-py", description: "ChatGPT web automation (DALL-E 3, file analysis) via playwright", status: "can" },
    SeedTool { name: "notebooklm", kind: "python-api", invocation: "notebooklm / /notebooklm", description: "Google NotebookLM programmatic API (podcast, summary, briefing)", status: "can" },
    SeedTool { name: "phone-use", kind: "adb", invocation: "$PF <cmd> via ADB WiFi / /phone-use", description: "Android phone control over ADB/SSH", status: "can" },
    SeedTool { name: "browser-automation", kind: "skill", invocation: "/browser-automation", description: "Browser login flows / key extraction → imperium-crawl interact", status: "can" },
    SeedTool { name: "computer-use", kind: "cli", invocation: "cu <cmd>", description: "X11 desktop control: screenshot/click/type/OCR", status: "can" },
    SeedTool { name: "transcribe", kind: "cli", invocation: "faster-whisper + yt-dlp / /transcribe", description: "Audio/video transcription", status: "can" },
    SeedTool { name: "graphify", kind: "skill", invocation: "/graphify <path>", description: "Any input → persistent knowledge graph", status: "can" },
    SeedTool { name: "hermes", kind: "binary", invocation: "~/.local/bin/hermes", description: "Hermes command center agent", status: "can" },
    SeedTool { name: "codex", kind: "binary", invocation: "~/.npm-global/bin/codex", description: "OpenAI Codex CLI (big-context coding)", status: "can" },
    SeedTool { name: "cursor", kind: "binary", invocation: "~/.local/bin/cursor", description: "Cursor CLI (AI coding)", status: "can" },
    SeedTool { name: "imperium-cloud", kind: "web-service", invocation: "HTTP to local PM2 / /imperium-cloud", description: "Unified infra API over 17+ free cloud providers", status: "unverified" },
    SeedTool { name: "vm-deploy", kind: "skill", invocation: "/vm-deploy", description: "Oracle Cloud VM deploy", status: "can" },
    SeedTool { name: "vm-up", kind: "skill", invocation: "/vm-up", description: "VM health check", status: "can" },
    SeedTool { name: "content-pipeline", kind: "skill", invocation: "/content-pipeline", description: "Social content production pipeline", status: "can" },
];

/// Agent names that legitimately exist in BOTH worlds (tool_records +
/// adapter_dossiers) — `adapter_ref` links by name.
const ADAPTER_NAMES: [&str; 5] = ["claude-code", "codex", "cursor", "antigravity", "hermes"];

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

/// One discovery hit from a scan source. Reconciliation groups by (name, kind).
#[derive(Debug, Clone)]
pub struct Discovered {
    pub name: String,
    pub kind: String,
    pub location: String,
    pub description: Option<String>,
    pub can_do: Vec<String>,
    pub cannot_do: Vec<String>,
    /// True for $PATH hits — preferred for the canonical invocation.
    pub is_path_hit: bool,
}

impl Discovered {
    fn new(name: impl Into<String>, kind: &str, location: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: kind.to_string(),
            location: location.into(),
            description: None,
            can_do: Vec::new(),
            cannot_do: Vec::new(),
            is_path_hit: false,
        }
    }
}

/// Explicit scan sources so tests stay hermetic (no real $HOME ever touched
/// in tests — fixtures pass their own dirs).
pub struct ScanSources {
    pub path_dirs: Vec<PathBuf>,
    pub skills_dirs: Vec<PathBuf>,
    pub capabilities_dirs: Vec<PathBuf>,
    pub checkout_roots: Vec<PathBuf>,
    /// Version-manager shim dirs — PATH entries living here, or realpathing
    /// here, are SKIPPED (every shim realpaths to one binary; name-based
    /// reconciliation would collapse unrelated tools).
    pub shim_denylist: Vec<PathBuf>,
}

impl ScanSources {
    pub fn from_env(home: &Path, checkouts_override: Option<PathBuf>) -> Self {
        // Only USER-installed bin dirs. A raw $PATH walk drags in every
        // system binary (/usr/bin zstdcat, zramctl, ... — ~3,700 rows of
        // noise on a real machine) and the register's whole purpose is
        // "agents know Pavle's tools, no wandering". System dirs are
        // excluded; AI-relevant tools live in user dirs, skills,
        // capabilities YAMLs, and source checkouts.
        let system_dirs = ["/usr/bin", "/usr/sbin", "/bin", "/sbin", "/usr/local/sbin"];
        let mut path_dirs: Vec<PathBuf> = std::env::var("PATH")
            .unwrap_or_default()
            .split(':')
            .filter(|s| !s.trim().is_empty())
            .filter(|s| {
                let under_home = Path::new(s).starts_with(home);
                let is_system = system_dirs.iter().any(|d| Path::new(s) == Path::new(d));
                under_home && !is_system
            })
            .map(PathBuf::from)
            .collect();
        for extra in [home.join(".local/bin"), home.join(".cargo/bin"), home.join("bin")] {
            if !path_dirs.contains(&extra) && extra.is_dir() {
                path_dirs.push(extra);
            }
        }
        // npm-global bin dirs (may or may not already be on PATH).
        let npm_global = home.join(".npm-global/bin");
        if !path_dirs.contains(&npm_global) {
            path_dirs.push(npm_global);
        }

        let mut shim_denylist = vec![
            home.join(".local/share/mise/shims"),
            home.join(".asdf/shims"),
            home.join(".nvm"),
        ];
        if let Ok(nvm_bin) = std::env::var("NVM_BIN") {
            if !nvm_bin.trim().is_empty() {
                shim_denylist.push(PathBuf::from(nvm_bin));
            }
        }

        Self {
            path_dirs,
            skills_dirs: vec![home.join(".claude/skills")],
            capabilities_dirs: vec![home.join(".imperium/capabilities")],
            checkout_roots: vec![checkouts_override.unwrap_or_else(|| home.join("projekti"))],
            shim_denylist,
        }
    }
}

fn is_under(p: &Path, root: &Path) -> bool {
    p.starts_with(root)
}

fn in_shim_denylist(p: &Path, denylist: &[PathBuf]) -> bool {
    if denylist.iter().any(|d| is_under(p, d)) {
        return true;
    }
    // An entry RESOLVING into a shim dir is skipped too.
    if let Ok(real) = std::fs::canonicalize(p) {
        if denylist.iter().any(|d| is_under(&real, d)) {
            return true;
        }
    }
    false
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

/// Scan $PATH-style bin dirs. Kind = `binary`. Shim dirs are denylisted;
/// within a (name) group, locations whose realpath is the SAME file are
/// deduped (symlink aliases) — different realpaths are all kept.
pub fn scan_path_dirs(dirs: &[PathBuf], shim_denylist: &[PathBuf]) -> Vec<Discovered> {
    let mut out = Vec::new();
    let mut seen_dirs: HashSet<PathBuf> = HashSet::new();
    for dir in dirs {
        if !seen_dirs.insert(dir.clone()) {
            continue; // duplicate PATH entry
        }
        if in_shim_denylist(dir, shim_denylist) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if !is_executable(&p) {
                continue;
            }
            if in_shim_denylist(&p, shim_denylist) {
                continue;
            }
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let mut d = Discovered::new(name, "binary", p.to_string_lossy());
            d.is_path_hit = true;
            out.push(d);
        }
    }
    out
}

/// Scan Claude-style skills dirs: each subdir with a SKILL.md → kind `skill`,
/// name + description from YAML frontmatter (fallback: dir name).
pub fn scan_skills_dirs(dirs: &[PathBuf]) -> Vec<Discovered> {
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            let manifest = p.join("SKILL.md");
            if !p.is_dir() || !manifest.is_file() {
                continue;
            }
            let dir_name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let (name, description) = std::fs::read_to_string(&manifest)
                .ok()
                .and_then(|body| parse_skill_frontmatter(&body))
                .map(|(n, d)| (n.unwrap_or_else(|| dir_name.clone()), d))
                .unwrap_or((dir_name.clone(), None));
            let mut d = Discovered::new(name, "skill", p.to_string_lossy());
            d.description = description;
            out.push(d);
        }
    }
    out
}

/// Lenient SKILL.md frontmatter parse: `---\nname: ...\ndescription: ...\n---`.
/// Returns (name, description); either may be absent.
pub fn parse_skill_frontmatter(body: &str) -> Option<(Option<String>, Option<String>)> {
    let rest = body.strip_prefix("---")?;
    let end = rest.find("\n---")?;
    let yaml: serde_yaml::Value = serde_yaml::from_str(&rest[..end]).ok()?;
    let get = |k: &str| {
        yaml.get(k)
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    Some((get("name"), get("description")))
}

/// Scan `~/.imperium/capabilities/*.yaml` agent snapshots (graceful if the
/// dir is missing). A file qualifies when it carries an `agent:` key. The
/// agent binary becomes a `binary` tool record with can/cannot lists;
/// adapter linkage happens at reconcile time.
pub fn scan_capabilities_dirs(dirs: &[PathBuf]) -> Vec<Discovered> {
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue; // missing dir — graceful skip
        };
        for entry in entries.flatten() {
            let p = entry.path();
            let ext_ok = p
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e == "yaml" || e == "yml")
                .unwrap_or(false);
            if !ext_ok || !p.is_file() {
                continue;
            }
            let Ok(body) = std::fs::read_to_string(&p) else {
                continue;
            };
            let Ok(yaml) = serde_yaml::from_str::<serde_yaml::Value>(&body) else {
                continue;
            };
            let Some(agent) = yaml.get("agent").and_then(|v| v.as_str()) else {
                continue; // not an agent snapshot (e.g. manifest.yaml)
            };
            let name = map_agent_name(agent);
            let binary = yaml.get("binary").and_then(|v| v.as_str());
            let mut d = Discovered::new(
                name,
                "binary",
                binary.unwrap_or(&p.to_string_lossy()).to_string(),
            );
            d.description = yaml
                .get("display_name")
                .and_then(|v| v.as_str())
                .map(String::from);
            d.can_do = yaml_str_list(&yaml, "can");
            d.cannot_do = yaml_str_list(&yaml, "cannot");
            out.push(d);
        }
    }
    out
}

pub(crate) fn map_agent_name(agent: &str) -> String {
    match agent {
        "claude" => "claude-code".to_string(),
        other => other.to_string(),
    }
}

fn yaml_str_list(yaml: &serde_yaml::Value, key: &str) -> Vec<String> {
    yaml.get(key)
        .and_then(|v| v.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// S3 DENY globs (PLAN.md §S3, inherited by §P1.3): never open a path whose
/// any component starts with `auth`, contains `token` / `secret` / `.env`,
/// or is a database file. Applied BEFORE opening anything under checkouts.
pub fn path_is_denied(p: &Path) -> bool {
    for comp in p.components() {
        let s = comp.as_os_str().to_string_lossy().to_lowercase();
        if s.starts_with("auth")
            || s.contains("token")
            || s.contains("secret")
            || s.contains(".env")
            || s.ends_with(".db")
            || s.ends_with(".sqlite")
            || s.ends_with(".sqlite3")
        {
            return true;
        }
    }
    false
}

const SKIP_DIRS: [&str; 8] = [
    "node_modules",
    "target",
    ".git",
    "dist",
    "build",
    ".venv",
    "venv",
    "__pycache__",
];

/// Shallow source-checkout scan: walk each root (max depth 4, heavy dirs
/// pruned), read recognizable tool manifests — package.json `bin`, Cargo.toml
/// `[[bin]]`, pyproject `[project.scripts]`. Kind = `binary` (same kind as
/// the PATH scan so a tool installed npm-global AND checked out reconciles
/// to ONE row).
pub fn scan_checkout_roots(roots: &[PathBuf]) -> Vec<Discovered> {
    let mut out = Vec::new();
    for root in roots {
        if !root.is_dir() {
            continue;
        }
        let walker = walkdir::WalkDir::new(root)
            .max_depth(4)
            .into_iter()
            .filter_entry(|e| {
                // Depth 0 is the scan root itself — never prune it (a TempDir
                // fixture or hidden checkout root would otherwise vanish).
                if e.depth() == 0 {
                    return true;
                }
                let name = e.file_name().to_string_lossy();
                if e.file_type().is_dir()
                    && (SKIP_DIRS.contains(&name.as_ref()) || name.starts_with('.'))
                {
                    return false;
                }
                // DENY globs prune whole subtrees before anything is opened —
                // checked against the path RELATIVE to the scan root so a
                // denied component in the root's own ancestry doesn't blind
                // (or falsely trip) the scan.
                let rel = e.path().strip_prefix(root).unwrap_or(e.path());
                !path_is_denied(rel)
            });
        for entry in walker.flatten() {
            if !entry.file_type().is_file() {
                continue;
            }
            let p = entry.path();
            if path_is_denied(p.strip_prefix(root).unwrap_or(p)) {
                continue; // defense in depth — never open denied paths
            }
            match p.file_name().and_then(|n| n.to_str()) {
                Some("package.json") => out.extend(scan_package_json(p)),
                Some("Cargo.toml") => out.extend(scan_cargo_toml(p)),
                Some("pyproject.toml") => out.extend(scan_pyproject(p)),
                _ => {}
            }
        }
    }
    out
}

fn checkout_location(manifest: &Path) -> String {
    manifest
        .parent()
        .unwrap_or(manifest)
        .to_string_lossy()
        .into_owned()
}

fn scan_package_json(p: &Path) -> Vec<Discovered> {
    let Ok(body) = std::fs::read_to_string(p) else {
        return vec![];
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) else {
        return vec![];
    };
    let description = json
        .get("description")
        .and_then(|v| v.as_str())
        .map(String::from);
    let loc = checkout_location(p);
    let mut out = Vec::new();
    match json.get("bin") {
        Some(serde_json::Value::Object(map)) => {
            for name in map.keys() {
                let mut d = Discovered::new(name, "binary", loc.clone());
                d.description = description.clone();
                out.push(d);
            }
        }
        Some(serde_json::Value::String(_)) => {
            // "bin": "./cli.js" — the bin name is the (unscoped) package name.
            if let Some(name) = json.get("name").and_then(|v| v.as_str()) {
                let unscoped = name.rsplit('/').next().unwrap_or(name);
                let mut d = Discovered::new(unscoped, "binary", loc);
                d.description = description;
                out.push(d);
            }
        }
        _ => {}
    }
    out
}

fn scan_cargo_toml(p: &Path) -> Vec<Discovered> {
    let Ok(body) = std::fs::read_to_string(p) else {
        return vec![];
    };
    let Ok(doc) = body.parse::<toml::Table>() else {
        return vec![];
    };
    let loc = checkout_location(p);
    let description = doc
        .get("package")
        .and_then(|pk| pk.get("description"))
        .and_then(|v| v.as_str())
        .map(String::from);
    let mut out = Vec::new();
    if let Some(bins) = doc.get("bin").and_then(|b| b.as_array()) {
        for bin in bins {
            if let Some(name) = bin.get("name").and_then(|v| v.as_str()) {
                let mut d = Discovered::new(name, "binary", loc.clone());
                d.description = description.clone();
                out.push(d);
            }
        }
    }
    out
}

fn scan_pyproject(p: &Path) -> Vec<Discovered> {
    let Ok(body) = std::fs::read_to_string(p) else {
        return vec![];
    };
    let Ok(doc) = body.parse::<toml::Table>() else {
        return vec![];
    };
    let loc = checkout_location(p);
    let project = doc.get("project");
    let description = project
        .and_then(|pr| pr.get("description"))
        .and_then(|v| v.as_str())
        .map(String::from);
    let mut out = Vec::new();
    if let Some(scripts) = project
        .and_then(|pr| pr.get("scripts"))
        .and_then(|s| s.as_table())
    {
        for name in scripts.keys() {
            let mut d = Discovered::new(name, "binary", loc.clone());
            d.description = description.clone();
            out.push(d);
        }
    }
    out
}

/// Run every scan source.
pub fn discover_all(sources: &ScanSources) -> Vec<Discovered> {
    let mut all = scan_path_dirs(&sources.path_dirs, &sources.shim_denylist);
    all.extend(scan_skills_dirs(&sources.skills_dirs));
    all.extend(scan_capabilities_dirs(&sources.capabilities_dirs));
    all.extend(scan_checkout_roots(&sources.checkout_roots));
    all
}

// ---------------------------------------------------------------------------
// Reconciliation — key = (name, kind)
// ---------------------------------------------------------------------------

/// Merge discoveries into upsert-ready rows against the existing register.
///
/// * Group by `(name, kind)` — one row per pair, ALL locations kept.
/// * Within a group, locations are deduped by realpath ONLY when two paths
///   resolve to the identical file (symlink aliases); different realpaths
///   (npm-global vs source checkout) all stay.
/// * Canonical invocation: an existing manual/seeded entry wins; else the
///   first $PATH hit; else the first location. The rest become alternates.
/// * Status / description: existing values are preserved (scan never
///   downgrades curated rows); new rows start `unverified`.
pub fn reconcile(
    discovered: Vec<Discovered>,
    existing: &[ToolRecordRow],
    dossier_names: &HashSet<String>,
) -> Vec<ToolRecordRow> {
    let existing_by_key: BTreeMap<(String, String), &ToolRecordRow> = existing
        .iter()
        .map(|r| ((r.name.clone(), r.kind.clone()), r))
        .collect();

    // Group discoveries.
    let mut groups: BTreeMap<(String, String), Vec<Discovered>> = BTreeMap::new();
    for d in discovered {
        groups
            .entry((d.name.clone(), d.kind.clone()))
            .or_default()
            .push(d);
    }

    let mut out = Vec::new();
    for ((name, kind), hits) in groups {
        let prior = existing_by_key.get(&(name.clone(), kind.clone()));

        // ---- locations: existing ∪ discovered, realpath-deduped (aliases only) ----
        let mut locations: Vec<String> = prior
            .map(|r| {
                r.locations
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        let mut seen_real: HashSet<PathBuf> = locations
            .iter()
            .map(|l| {
                std::fs::canonicalize(l).unwrap_or_else(|_| PathBuf::from(l))
            })
            .collect();
        let mut first_path_hit: Option<String> = None;
        for h in &hits {
            let real = std::fs::canonicalize(&h.location)
                .unwrap_or_else(|_| PathBuf::from(&h.location));
            if h.is_path_hit && first_path_hit.is_none() {
                first_path_hit = Some(h.location.clone());
            }
            if seen_real.insert(real) {
                locations.push(h.location.clone());
            }
        }

        // ---- canonical invocation ----
        let prior_canonical = prior
            .filter(|r| r.source == "manual")
            .and_then(|r| r.invocation.get("canonical"))
            .and_then(|v| v.as_str())
            .map(String::from);
        let canonical = prior_canonical
            .or(first_path_hit)
            .or_else(|| locations.first().cloned());
        let alternates: Vec<String> = locations
            .iter()
            .filter(|l| Some(l.as_str()) != canonical.as_deref())
            .cloned()
            .collect();
        let invocation = serde_json::json!({
            "canonical": canonical,
            "alternates": alternates,
        });

        // ---- merged can/cannot from capability snapshots ----
        let mut can_do: Vec<String> = prior
            .and_then(|r| r.can_do.as_array().cloned())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        let mut cannot_do: Vec<String> = prior
            .and_then(|r| r.cannot_do.as_array().cloned())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        for h in &hits {
            for c in &h.can_do {
                if !can_do.contains(c) {
                    can_do.push(c.clone());
                }
            }
            for c in &h.cannot_do {
                if !cannot_do.contains(c) {
                    cannot_do.push(c.clone());
                }
            }
        }

        let description = prior
            .and_then(|r| r.description.clone())
            .or_else(|| hits.iter().find_map(|h| h.description.clone()));

        let adapter_ref = prior
            .and_then(|r| r.adapter_ref.clone())
            .or_else(|| {
                (dossier_names.contains(&name) || ADAPTER_NAMES.contains(&name.as_str()))
                    .then(|| name.clone())
            });

        let mut row = match prior {
            Some(r) => (*r).clone(),
            None => ToolRecordRow::new(&name, &kind),
        };
        row.invocation = invocation;
        row.locations = serde_json::json!(locations);
        row.can_do = serde_json::json!(can_do);
        row.cannot_do = serde_json::json!(cannot_do);
        row.description = description;
        row.adapter_ref = adapter_ref;
        // Existing source/status preserved by the clone; fresh rows are
        // source=scan, status=unverified from ToolRecordRow::new.
        out.push(row);
    }
    out
}

// ---------------------------------------------------------------------------
// Command runners
// ---------------------------------------------------------------------------

async fn run_scan(args: ToolScanArgs) -> anyhow::Result<()> {
    // Batch writer: stand down during db unify (non-fatal). Dry-run is
    // read-only and may proceed.
    if !args.dry_run && crate::commands::brain::refuse_if_maintenance_locked("tool scan") {
        return Ok(());
    }

    let home = altevra_core::home_dir();
    let sources = ScanSources::from_env(&home, args.checkouts.clone());
    let discovered = discover_all(&sources);

    let pool = create_pool(&args.db.to_string_lossy()).await?;
    run_migrations(&pool).await?;
    let repo = ToolRecordsRepository::new(&pool);
    let existing = repo.list(None, None).await?;
    let dossier_names: HashSet<String> = AdapterDossiersRepository::new(&pool)
        .list()
        .await?
        .into_iter()
        .map(|d| d.tool_name)
        .collect();

    let rows = reconcile(discovered, &existing, &dossier_names);

    if args.dry_run {
        println!("DRY-RUN — {} tool record(s) would be upserted:", rows.len());
        for r in &rows {
            println!(
                "  {} ({}) — {} location(s), status={}, source={}",
                r.name,
                r.kind,
                r.locations.as_array().map(|a| a.len()).unwrap_or(0),
                r.status,
                r.source,
            );
        }
        return Ok(());
    }

    let mut sightings = 0usize;
    for r in &rows {
        sightings += repo.upsert(r).await?;
    }
    println!(
        "Scanned {} source group(s) → upserted {} tool record(s) ({} secret sighting(s) redacted + logged).",
        rows.len(),
        rows.len(),
        sightings,
    );
    Ok(())
}

async fn run_list(args: ToolListArgs) -> anyhow::Result<()> {
    let pool = create_pool(&args.db.to_string_lossy()).await?;
    run_migrations(&pool).await?;
    let rows = ToolRecordsRepository::new(&pool)
        .list(args.kind.as_deref(), args.status.as_deref())
        .await?;

    if args.json {
        let entries: Vec<_> = rows.iter().map(tool_row_json).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "count": entries.len(),
                "tools": entries,
            }))?
        );
    } else if rows.is_empty() {
        println!("No tools registered (run `altevra tool seed` / `altevra tool scan`).");
    } else {
        println!("{} tool(s):", rows.len());
        for r in &rows {
            println!(
                "  [{:10}] {:24} {:10} {}",
                r.status,
                r.name,
                r.kind,
                r.invocation
                    .get("canonical")
                    .and_then(|v| v.as_str())
                    .unwrap_or("-"),
            );
        }
    }
    Ok(())
}

pub(crate) fn tool_row_json(r: &ToolRecordRow) -> serde_json::Value {
    serde_json::json!({
        "name": r.name,
        "kind": r.kind,
        "display_name": r.display_name,
        "description": r.description,
        "invocation": r.invocation,
        "locations": r.locations,
        "can_do": r.can_do,
        "cannot_do": r.cannot_do,
        "unverified": r.unverified,
        "requires_session": r.requires_session,
        "status": r.status,
        "last_verified_at": r.last_verified_at,
        "categories": r.categories,
        "source": r.source,
        "adapter_ref": r.adapter_ref,
    })
}

async fn run_register(args: ToolRegisterArgs) -> anyhow::Result<()> {
    if crate::commands::brain::refuse_if_maintenance_locked("tool register") {
        return Ok(());
    }
    let pool = create_pool(&args.db.to_string_lossy()).await?;
    run_migrations(&pool).await?;
    let repo = ToolRecordsRepository::new(&pool);

    // Manual registration merges over an existing row, never clobbers
    // discovered locations.
    let mut row = repo
        .get(&args.name, &args.kind)
        .await?
        .unwrap_or_else(|| ToolRecordRow::new(&args.name, &args.kind));
    if let Some(d) = &args.description {
        row.description = Some(d.clone());
    }
    if let Some(dn) = &args.display_name {
        row.display_name = Some(dn.clone());
    }
    if let Some(inv) = &args.invocation {
        let alternates = row
            .invocation
            .get("alternates")
            .cloned()
            .unwrap_or(serde_json::json!([]));
        row.invocation = serde_json::json!({"canonical": inv, "alternates": alternates});
    }
    row.source = "manual".to_string();
    if ADAPTER_NAMES.contains(&args.name.as_str()) {
        row.adapter_ref.get_or_insert_with(|| args.name.clone());
    }
    let sightings = repo.upsert(&row).await?;
    println!(
        "Registered {} ({}){}",
        args.name,
        args.kind,
        if sightings > 0 {
            format!(" — {sightings} secret(s) redacted + logged")
        } else {
            String::new()
        }
    );
    Ok(())
}

async fn run_verify(args: ToolVerifyArgs) -> anyhow::Result<()> {
    if crate::commands::brain::refuse_if_maintenance_locked("tool verify") {
        return Ok(());
    }
    let pool = create_pool(&args.db.to_string_lossy()).await?;
    run_migrations(&pool).await?;
    let found = ToolRecordsRepository::new(&pool)
        .set_status(&args.name, &args.kind, &args.status)
        .await?;
    if found {
        println!("Verified {} ({}) → {}", args.name, args.kind, args.status);
    } else {
        anyhow::bail!(
            "no tool_record for ({}, {}) — register or scan it first",
            args.name,
            args.kind
        );
    }
    Ok(())
}

async fn run_seed(args: ToolSeedArgs) -> anyhow::Result<()> {
    if !args.dry_run && crate::commands::brain::refuse_if_maintenance_locked("tool seed") {
        return Ok(());
    }
    let pool = create_pool(&args.db.to_string_lossy()).await?;
    run_migrations(&pool).await?;
    if args.dry_run {
        println!("DRY-RUN — {} seed tool(s) would be upserted:", SEED_TOOLS.len());
        for s in &SEED_TOOLS {
            println!("  {} ({}) status={}", s.name, s.kind, s.status);
        }
        return Ok(());
    }
    let n = seed_tools(&pool).await?;
    println!("Seeded {n} priority tool(s) (idempotent).");
    Ok(())
}

/// Idempotent SEED_TOOLS upsert. Preserves any locations a previous scan
/// recorded; (re)asserts the curated invocation/description/status.
pub async fn seed_tools(pool: &sqlx::SqlitePool) -> anyhow::Result<usize> {
    let repo = ToolRecordsRepository::new(pool);
    for s in &SEED_TOOLS {
        let mut row = repo
            .get(s.name, s.kind)
            .await?
            .unwrap_or_else(|| ToolRecordRow::new(s.name, s.kind));
        let alternates = row
            .invocation
            .get("alternates")
            .cloned()
            .unwrap_or(serde_json::json!([]));
        row.invocation = serde_json::json!({"canonical": s.invocation, "alternates": alternates});
        row.description = Some(s.description.to_string());
        row.status = s.status.to_string();
        row.source = "manual".to_string();
        if ADAPTER_NAMES.contains(&s.name) {
            row.adapter_ref.get_or_insert_with(|| s.name.to_string());
        }
        repo.upsert(&row).await?;
    }
    Ok(SEED_TOOLS.len())
}

// ---------------------------------------------------------------------------
// Tests — hermetic (TempDir fixtures + per-test temp DBs only)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_exec(dir: &Path, name: &str, body: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        p
    }

    async fn temp_pool(tmp: &TempDir) -> sqlx::SqlitePool {
        let db = tmp.path().join("altevra.db");
        let pool = create_pool(&db.to_string_lossy()).await.unwrap();
        run_migrations(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn same_tool_two_locations_one_row_all_locations() {
        // The motivating §P1 case: one tool installed in two places (e.g.
        // npm-global AND a source checkout) → ONE row, BOTH locations.
        let tmp = TempDir::new().unwrap();
        let bin_a = tmp.path().join("npm-global/bin");
        let bin_b = tmp.path().join("other/bin");
        write_exec(&bin_a, "mytool", "#!/bin/sh\necho a");
        write_exec(&bin_b, "mytool", "#!/bin/sh\necho b");

        let discovered = scan_path_dirs(&[bin_a.clone(), bin_b.clone()], &[]);
        assert_eq!(discovered.len(), 2);
        let rows = reconcile(discovered, &[], &HashSet::new());
        assert_eq!(rows.len(), 1, "(name,kind) reconciliation → one row");
        let locs = rows[0].locations.as_array().unwrap();
        assert_eq!(locs.len(), 2, "ALL locations kept: {locs:?}");
        // Canonical = first PATH hit; the other is an alternate.
        assert_eq!(
            rows[0].invocation["canonical"],
            bin_a.join("mytool").to_string_lossy().to_string()
        );
        assert_eq!(rows[0].invocation["alternates"].as_array().unwrap().len(), 1);

        // Persist + re-scan: idempotent (still one row, two locations).
        let pool = temp_pool(&tmp).await;
        let repo = ToolRecordsRepository::new(&pool);
        repo.upsert(&rows[0]).await.unwrap();
        let existing = repo.list(None, None).await.unwrap();
        let rows2 = reconcile(
            scan_path_dirs(&[bin_a, bin_b], &[]),
            &existing,
            &HashSet::new(),
        );
        repo.upsert(&rows2[0]).await.unwrap();
        let all = repo.list(None, None).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].locations.as_array().unwrap().len(), 2);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_alias_dedups_to_one_location() {
        // Realpath is used ONLY to dedup identical-file PATH aliases.
        let tmp = TempDir::new().unwrap();
        let bin_a = tmp.path().join("a");
        let bin_b = tmp.path().join("b");
        let real = write_exec(&bin_a, "linked", "#!/bin/sh\n");
        std::fs::create_dir_all(&bin_b).unwrap();
        std::os::unix::fs::symlink(&real, bin_b.join("linked")).unwrap();

        let rows = reconcile(
            scan_path_dirs(&[bin_a, bin_b], &[]),
            &[],
            &HashSet::new(),
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].locations.as_array().unwrap().len(),
            1,
            "symlink alias of the SAME file must dedup"
        );
    }

    #[test]
    fn shim_dirs_are_excluded() {
        // Version-manager shims (mise/asdf/nvm) all realpath to one shim
        // binary — name-based reconciliation would corrupt; the dir is
        // denylisted, and entries RESOLVING into it are skipped too.
        let tmp = TempDir::new().unwrap();
        let shims = tmp.path().join(".local/share/mise/shims");
        write_exec(&shims, "node", "#!/bin/sh\n");
        write_exec(&shims, "imperium-crawl", "#!/bin/sh\n");
        let normal = tmp.path().join("bin");
        write_exec(&normal, "realtool", "#!/bin/sh\n");
        // A non-shim dir entry that SYMLINKS into the shim dir is skipped too.
        #[cfg(unix)]
        std::os::unix::fs::symlink(shims.join("node"), normal.join("node")).unwrap();

        let denylist = vec![shims.clone()];
        let found = scan_path_dirs(&[shims, normal], &denylist);
        let names: Vec<_> = found.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["realtool"], "shim entries must be excluded: {names:?}");
    }

    #[test]
    fn skills_dir_parses_frontmatter() {
        let tmp = TempDir::new().unwrap();
        let skill = tmp.path().join("skills/graphify");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: graphify\ndescription: \"any input to knowledge graph\"\n---\n# body\n",
        )
        .unwrap();
        // A dir without SKILL.md is ignored.
        std::fs::create_dir_all(tmp.path().join("skills/not-a-skill")).unwrap();

        let found = scan_skills_dirs(&[tmp.path().join("skills")]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "graphify");
        assert_eq!(found[0].kind, "skill");
        assert_eq!(
            found[0].description.as_deref(),
            Some("any input to knowledge graph")
        );
    }

    #[tokio::test]
    async fn same_name_different_kind_two_rows() {
        // "codex" as a skill AND a binary → two rows, never merged.
        let tmp = TempDir::new().unwrap();
        let bin = tmp.path().join("bin");
        write_exec(&bin, "codex", "#!/bin/sh\n");
        let skills = tmp.path().join("skills/codex");
        std::fs::create_dir_all(&skills).unwrap();
        std::fs::write(skills.join("SKILL.md"), "---\nname: codex\n---\n").unwrap();

        let mut discovered = scan_path_dirs(&[bin], &[]);
        discovered.extend(scan_skills_dirs(&[tmp.path().join("skills")]));
        let rows = reconcile(discovered, &[], &HashSet::new());
        assert_eq!(rows.len(), 2);

        let pool = temp_pool(&tmp).await;
        let repo = ToolRecordsRepository::new(&pool);
        for r in &rows {
            repo.upsert(r).await.unwrap();
        }
        let by_name = repo.get_by_name("codex").await.unwrap();
        assert_eq!(by_name.len(), 2);
        // codex is an adapter-world name → linked by adapter_ref on both.
        assert!(by_name.iter().all(|r| r.adapter_ref.as_deref() == Some("codex")));
    }

    #[tokio::test]
    async fn capabilities_yaml_with_bearer_token_redacted_and_sighted() {
        // §P1 gate: yaml-with-embedded-bearer-token → redacted row + sighting.
        let tmp = TempDir::new().unwrap();
        let caps = tmp.path().join("capabilities");
        std::fs::create_dir_all(&caps).unwrap();
        std::fs::write(
            caps.join("testbot.yaml"),
            "agent: testbot\n\
             display_name: \"Test Bot — Authorization: Bearer abcdefghijklmnop0123456789\"\n\
             binary: /usr/local/bin/testbot\n\
             can:\n  - code.read\ncannot:\n  - cron.create\n",
        )
        .unwrap();

        let found = scan_capabilities_dirs(&[caps]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "testbot");
        assert_eq!(found[0].can_do, vec!["code.read"]);

        let pool = temp_pool(&tmp).await;
        let repo = ToolRecordsRepository::new(&pool);
        let rows = reconcile(found, &[], &HashSet::new());
        let sightings = repo.upsert(&rows[0]).await.unwrap();
        assert!(sightings >= 1, "bearer token must be sighted");

        let got = repo.get("testbot", "binary").await.unwrap().unwrap();
        assert!(
            !got.description.as_deref().unwrap_or("").contains("abcdefghijklmnop"),
            "token leaked into description: {:?}",
            got.description
        );
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM secret_sightings WHERE source_ref = 'tool_record:testbot/binary'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(count >= 1, "sighting must be logged");
    }

    #[test]
    fn capabilities_dir_missing_is_graceful() {
        let found = scan_capabilities_dirs(&[PathBuf::from("/nonexistent/zzz-altevra-test")]);
        assert!(found.is_empty());
    }

    #[test]
    fn checkout_scan_finds_manifests_and_respects_deny_globs() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("cat/myrepo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(
            repo.join("package.json"),
            r#"{"name": "@scope/imperium-crawl", "description": "crawler", "bin": {"imperium-crawl": "./cli.js"}}"#,
        )
        .unwrap();
        let rs = tmp.path().join("cat/rustrepo");
        std::fs::create_dir_all(&rs).unwrap();
        std::fs::write(
            rs.join("Cargo.toml"),
            "[package]\nname = \"x\"\ndescription = \"rust tool\"\n[[bin]]\nname = \"rusttool\"\npath = \"src/main.rs\"\n",
        )
        .unwrap();
        let py = tmp.path().join("cat/pyrepo");
        std::fs::create_dir_all(&py).unwrap();
        std::fs::write(
            py.join("pyproject.toml"),
            "[project]\nname = \"p\"\ndescription = \"py tool\"\n[project.scripts]\npytool = \"p.cli:main\"\n",
        )
        .unwrap();
        // DENY: a manifest under a secrets-ish dir must NEVER be opened.
        let denied = tmp.path().join("cat/my-secrets-repo");
        std::fs::create_dir_all(&denied).unwrap();
        std::fs::write(
            denied.join("package.json"),
            r#"{"name": "evil", "bin": {"eviltool": "./x.js"}}"#,
        )
        .unwrap();
        let denied2 = tmp.path().join("cat/authstuff");
        std::fs::create_dir_all(&denied2).unwrap();
        std::fs::write(
            denied2.join("package.json"),
            r#"{"name": "evil2", "bin": {"eviltool2": "./x.js"}}"#,
        )
        .unwrap();

        let found = scan_checkout_roots(&[tmp.path().to_path_buf()]);
        let names: HashSet<_> = found.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains("imperium-crawl"));
        assert!(names.contains("rusttool"));
        assert!(names.contains("pytool"));
        assert!(!names.contains("eviltool"), "S3 DENY glob **/*secret* violated");
        assert!(!names.contains("eviltool2"), "S3 DENY glob **/auth* violated");
    }

    #[test]
    fn deny_glob_predicate() {
        assert!(path_is_denied(Path::new("/x/auth-config/p.json")));
        assert!(path_is_denied(Path::new("/x/my_tokens/p.json")));
        assert!(path_is_denied(Path::new("/x/repo/.env.local")));
        assert!(path_is_denied(Path::new("/x/repo/data.sqlite")));
        assert!(path_is_denied(Path::new("/x/repo/app.db")));
        assert!(!path_is_denied(Path::new("/x/repo/package.json")));
    }

    #[tokio::test]
    async fn seed_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let pool = temp_pool(&tmp).await;
        let n1 = seed_tools(&pool).await.unwrap();
        let rows1 = ToolRecordsRepository::new(&pool).list(None, None).await.unwrap();
        let n2 = seed_tools(&pool).await.unwrap();
        let rows2 = ToolRecordsRepository::new(&pool).list(None, None).await.unwrap();
        assert_eq!(n1, 15);
        assert_eq!(n2, 15);
        assert_eq!(rows1.len(), 15, "first seed lands all 15");
        assert_eq!(rows2.len(), 15, "second seed must NOT duplicate");
        for (a, b) in rows1.iter().zip(rows2.iter()) {
            assert_eq!((&a.name, &a.kind, &a.status), (&b.name, &b.kind, &b.status));
            assert_eq!(a.invocation, b.invocation);
        }
        // Kind/status straight from the ASSESSMENT §4 table.
        let repo = ToolRecordsRepository::new(&pool);
        let ic = repo.get("imperium-crawl", "cli").await.unwrap().unwrap();
        assert_eq!(ic.status, "can");
        assert_eq!(ic.source, "manual");
        let cloud = repo.get("imperium-cloud", "web-service").await.unwrap().unwrap();
        assert_eq!(cloud.status, "unverified");
        // Both-worlds entities carry adapter_ref.
        let hermes = repo.get("hermes", "binary").await.unwrap().unwrap();
        assert_eq!(hermes.adapter_ref.as_deref(), Some("hermes"));
    }

    #[tokio::test]
    async fn seed_then_scan_preserves_curated_invocation_and_status() {
        // A scan after seeding merges locations but never downgrades the
        // curated (manual) canonical invocation or status.
        let tmp = TempDir::new().unwrap();
        let pool = temp_pool(&tmp).await;
        seed_tools(&pool).await.unwrap();
        let repo = ToolRecordsRepository::new(&pool);

        let bin = tmp.path().join("bin");
        let exe = write_exec(&bin, "codex", "#!/bin/sh\n");
        let existing = repo.list(None, None).await.unwrap();
        let rows = reconcile(scan_path_dirs(&[bin], &[]), &existing, &HashSet::new());
        let codex = rows.iter().find(|r| r.name == "codex").unwrap();
        repo.upsert(codex).await.unwrap();

        let got = repo.get("codex", "binary").await.unwrap().unwrap();
        assert_eq!(got.status, "can", "scan must not downgrade seeded status");
        assert_eq!(got.source, "manual", "curated source preserved");
        assert_eq!(
            got.invocation["canonical"], "~/.npm-global/bin/codex",
            "curated canonical invocation wins over the scan hit"
        );
        let locs: Vec<_> = got
            .locations
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(
            locs.contains(&exe.to_string_lossy().as_ref()),
            "scan location merged in: {locs:?}"
        );
    }

    #[tokio::test]
    async fn dry_run_scan_writes_nothing() {
        let tmp = TempDir::new().unwrap();
        let bin = tmp.path().join("bin");
        write_exec(&bin, "drytool", "#!/bin/sh\n");
        let db = tmp.path().join("altevra.db");

        // Simulate the dry-run path: discover + reconcile, no upsert.
        let pool = create_pool(&db.to_string_lossy()).await.unwrap();
        run_migrations(&pool).await.unwrap();
        let repo = ToolRecordsRepository::new(&pool);
        let rows = reconcile(scan_path_dirs(&[bin], &[]), &[], &HashSet::new());
        assert_eq!(rows.len(), 1);
        // dry-run prints only — the register stays empty.
        assert!(repo.list(None, None).await.unwrap().is_empty());
    }
}

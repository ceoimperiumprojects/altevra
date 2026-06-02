//! `altevra capture <file.md>` — ingest a real Obsidian/markdown note into the
//! second brain through the PreWriteSafetyGate, then persist it as a `learning`
//! (which auto-indexes into `object_index` + FTS so `recall`/`search_memory`
//! find it immediately, T-INV14).
//!
//! This is the real-world write path the vision (CLAUDE.md §3) calls for:
//! Pavle's decisions / learnings / daily notes flow in, get secret+PII redacted,
//! get a domain (auto from path, overridable), and become recallable. SI-7 / R11
//! honesty is preserved:
//!   * `guard_text` redacts secrets + PII before anything is stored.
//!   * a credential-class secret (PEM / db-url) → REFUSE capture (never store).
//!   * a high-water domain (personal/health/relationship/…) escalates sensitivity
//!     to Restricted (same rule as `ingest_guard`), so personal notes can't
//!     default-down and leak.

use altevra_core::domain::Domain;
use altevra_core::security::Sensitivity;
use altevra_db::{create_pool, run_migrations, LearningRow, LearningsRepository};
use altevra_secrets::guard_text;
use clap::Args;
use std::path::{Path, PathBuf};

#[derive(Args)]
pub struct CaptureArgs {
    /// Markdown file to capture (e.g. a note from ~/Obsidian/Imperium/).
    pub file: PathBuf,
    /// Domain override. If omitted, inferred from the file path (Memory/People →
    /// relationship, health → health, …) else `business`.
    #[arg(long)]
    pub domain: Option<String>,
    /// Declared sensitivity floor (`public`|`shareable`|`internal`|`confidential`|
    /// `secret`|`restricted`). The guard may raise it; never lowers below this.
    #[arg(long, default_value = "internal")]
    pub sensitivity: String,
    /// Title override (default: first `# heading` in the file, else the file stem).
    #[arg(long)]
    pub title: Option<String>,
    /// Comma-separated category tags (governed taxonomy). At least the inferred
    /// domain is always added so TAG-1 (no untagged object) holds.
    #[arg(long, value_delimiter = ',')]
    pub categories: Vec<String>,
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
    #[arg(long)]
    pub json: bool,
}

pub async fn run(args: CaptureArgs) -> anyhow::Result<()> {
    let raw = std::fs::read_to_string(&args.file)
        .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", args.file.display()))?;
    if raw.trim().is_empty() {
        anyhow::bail!("{} is empty — nothing to capture", args.file.display());
    }

    // Domain: explicit flag wins; else infer from the path.
    let domain: Domain = match args.domain.as_deref() {
        Some(d) => d
            .parse()
            .map_err(|_| anyhow::anyhow!("unknown --domain '{d}'"))?,
        None => infer_domain(&args.file),
    };
    let declared: Sensitivity = args
        .sensitivity
        .parse()
        .map_err(|_| anyhow::anyhow!("unknown --sensitivity '{}'", args.sensitivity))?;

    // ---- the safety gate ----
    let guarded = guard_text(&raw, declared);

    // A credential-class sighting (PEM / db-url) must NEVER be stored (§2.5 / R11).
    if guarded.sightings.iter().any(|s| s.action == "rejected") {
        anyhow::bail!(
            "refusing to capture {}: a credential-class secret was detected. \
             Remove it (or store it via `altevra secrets set`) and retry.",
            args.file.display()
        );
    }

    // High-water domain escalation — personal/health/relationship/… → Restricted,
    // mirroring ingest_guard so a personal note can't default-down and leak.
    let mut sensitivity = guarded.sensitivity.clone();
    if domain.is_high_water() {
        sensitivity = sensitivity.combine(&Sensitivity::Restricted);
    }

    let title = args
        .title
        .clone()
        .or_else(|| first_heading(&raw))
        .unwrap_or_else(|| file_stem(&args.file));

    // Tags: always include the domain so TAG-1 holds; merge any --categories.
    let mut cats: Vec<String> = vec![domain.to_string()];
    for c in &args.categories {
        let c = c.trim();
        if !c.is_empty() && !cats.contains(&c.to_string()) {
            cats.push(c.to_string());
        }
    }
    let categories_json = serde_json::to_string(&cats)?;

    // Deterministic-ish id: capture-<stem>-<8-char content hash> so re-capturing
    // an edited note makes a new row, but the same note twice collides predictably.
    let id = format!(
        "capture-{}-{}",
        slugify(&file_stem(&args.file)),
        short_hash(&raw)
    );

    let row = LearningRow {
        id: id.clone(),
        title: title.clone(),
        body: guarded.value.clone(),
        status: "active".into(),
        domain: domain.to_string(),
        scope: None,
        sensitivity: sensitivity.to_string(),
        provenance: format!(
            "{{\"origin\":\"pavle_direct\",\"imported_from\":{}}}",
            serde_json::to_string(&args.file.display().to_string())?
        ),
        redaction_status: guarded.redaction_status.to_string(),
        categories: categories_json,
        tags: serde_json::to_string(&cats)?,
        confidence: "high".into(),
    };

    let pool = create_pool(&args.db.to_string_lossy()).await?;
    run_migrations(&pool).await?;
    LearningsRepository::new(&pool).insert(&row).await?;

    let redacted_n = guarded.sightings.len();
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "id": id,
                "title": title,
                "domain": domain.to_string(),
                "sensitivity": sensitivity.to_string(),
                "redaction_status": guarded.redaction_status.to_string(),
                "redactions": redacted_n,
                "risk_tags": guarded.risk_tags.iter().map(|t| t.to_string()).collect::<Vec<_>>(),
                "categories": cats,
                "bytes": guarded.value.len(),
            }))?
        );
    } else {
        println!("Captured '{title}' [{id}]");
        println!(
            "  domain={domain}  sensitivity={sensitivity}  redaction={}",
            guarded.redaction_status
        );
        if redacted_n > 0 {
            println!("  ⚠ {redacted_n} secret/PII redaction(s) applied before storage");
        }
        println!("  categories: {}", cats.join(", "));
        println!("  → searchable now: altevra recall \"<term>\"");
    }
    Ok(())
}

/// Infer a domain from the note's path — keyless auto-categorization (§3.2). The
/// full LLM classifier is a later upgrade; this covers the Imperium vault layout.
fn infer_domain(path: &Path) -> Domain {
    let p = path.to_string_lossy().to_lowercase();
    // High-water first (most-restrictive wins).
    if p.contains("health") || p.contains("zdravlje") {
        Domain::Health
    } else if p.contains("relationship")
        || p.contains("people")
        || p.contains("elena")
        || p.contains("family")
    {
        Domain::Relationship
    } else if p.contains("financ") || p.contains("invoice") || p.contains("budget") {
        Domain::Financial
    } else if p.contains("legal") || p.contains("contract") || p.contains("ugovor") {
        Domain::Legal
    } else if p.contains("client") || p.contains("klijent") {
        Domain::Client
    } else if p.contains("personal") || p.contains("lično") || p.contains("licno") {
        Domain::Personal
    } else if p.contains("/projects/") || p.contains("/project") {
        Domain::Project
    } else {
        Domain::Business
    }
}

/// First `# heading` line, trimmed of leading `#` and spaces.
fn first_heading(content: &str) -> Option<String> {
    content.lines().find_map(|l| {
        let t = l.trim_start();
        t.strip_prefix("# ").map(|h| h.trim().to_string())
    })
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("note")
        .to_string()
}

fn slugify(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// 8-char hex of a FNV-1a hash — no crypto needed, just a stable content id.
fn short_hash(s: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:08x}", (h & 0xffffffff) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn infer_domain_from_path_is_high_water_aware() {
        assert_eq!(
            infer_domain(Path::new("/home/pavle/Obsidian/Imperium/Memory/People.md")),
            Domain::Relationship
        );
        assert_eq!(
            infer_domain(Path::new("/x/health/checkup.md")),
            Domain::Health
        );
        assert_eq!(
            infer_domain(Path::new("/x/Projects/altevra/README.md")),
            Domain::Project
        );
        assert_eq!(infer_domain(Path::new("/x/Decisions.md")), Domain::Business);
    }

    #[test]
    fn first_heading_and_stem_fallback() {
        assert_eq!(
            first_heading("---\nx: 1\n---\n# The Title\nbody"),
            Some("The Title".into())
        );
        assert_eq!(first_heading("no heading here"), None);
        assert_eq!(file_stem(Path::new("/a/b/my-note.md")), "my-note");
    }

    #[tokio::test]
    async fn capture_redacts_secret_and_makes_recallable() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("a.db");
        let note = tmp.path().join("note.md");
        // A note with a fake API-key-shaped secret + recoverable prose.
        let fake = concat!("sk-", "live", "ABCDEFGHIJKLMNOPQRSTUVWX0123");
        std::fs::write(
            &note,
            format!("# GTM Decision\n\nWe will target Florida surplus buyers. key={fake}\n"),
        )
        .unwrap();

        run(CaptureArgs {
            file: note,
            domain: Some("business".into()),
            sensitivity: "internal".into(),
            title: None,
            categories: vec!["gtm".into()],
            db: db.clone(),
            json: true,
        })
        .await
        .unwrap();

        // The captured learning must be (a) present, (b) redacted (no raw key),
        // (c) recallable by its prose.
        let pool = create_pool(&db.to_string_lossy()).await.unwrap();
        let idx = altevra_db::ObjectIndexRepository::new(&pool);
        let cands = idx.candidates(None).await.unwrap();
        assert_eq!(cands.len(), 1, "one learning indexed");
        let learnings = altevra_db::LearningsRepository::new(&pool);
        let got = learnings.get(&cands[0].id).await.unwrap().unwrap();
        assert!(!got.body.contains("sk-live"), "secret must be redacted");
        assert!(got.body.contains("Florida surplus"), "prose preserved");
        assert_eq!(got.title, "GTM Decision");
    }
}

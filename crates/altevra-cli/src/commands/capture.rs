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
use altevra_vault::{parse_sections, section_conformance};
use clap::Args;
use sqlx::SqlitePool;
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
    /// ATOMIZE: split the file into its `## ` sections and capture each as its own
    /// object (decision/learning/person/note — type inferred from the filename).
    /// Auto-enabled for known aggregates under Memory/ with ≥2 sections; this flag
    /// forces it on. `--no-atomize` forces the legacy whole-file path.
    #[arg(long, conflicts_with = "no_atomize")]
    pub atomize: bool,
    /// Force the legacy whole-file capture even for a multi-section aggregate.
    #[arg(long)]
    pub no_atomize: bool,
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

    let pool = create_pool(&args.db.to_string_lossy()).await?;
    run_migrations(&pool).await?;

    // Decide the path: explicit --atomize, or auto when the file is a known
    // aggregate (Memory/Decisions|Learnings|People|…) with ≥2 `## ` sections.
    // --no-atomize always forces the legacy whole-file path (back-compat).
    let sections = parse_sections(&raw);
    let want_atomize = !args.no_atomize
        && (args.atomize || (should_auto_atomize(&args.file) && sections.len() >= 2));

    if want_atomize {
        return run_atomize(&pool, &args, &raw, sections, domain, declared).await;
    }
    run_whole_file(&pool, &args, &raw, domain, declared).await
}

/// Legacy whole-file capture: one note → one `learning` (unchanged behaviour).
async fn run_whole_file(
    pool: &SqlitePool,
    args: &CaptureArgs,
    raw: &str,
    domain: Domain,
    declared: Sensitivity,
) -> anyhow::Result<()> {
    // ---- the safety gate ----
    let guarded = guard_text(raw, declared);

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
        .or_else(|| first_heading(raw))
        .unwrap_or_else(|| file_stem(&args.file));

    // Tags: always include the domain so TAG-1 holds; merge any --categories.
    let cats = build_categories(&domain, &args.categories, None);
    let categories_json = serde_json::to_string(&cats)?;

    // Deterministic-ish id: capture-<stem>-<8-char content hash> so re-capturing
    // an edited note makes a new row, but the same note twice collides predictably.
    let id = format!(
        "capture-{}-{}",
        slugify(&file_stem(&args.file)),
        short_hash(raw)
    );

    let row = LearningRow {
        id: id.clone(),
        title: title.clone(),
        body: guarded.value.clone(),
        status: "active".into(),
        domain: domain.to_string(),
        scope: None,
        sensitivity: sensitivity.to_string(),
        provenance: provenance_json(&args.file, None)?,
        redaction_status: guarded.redaction_status.to_string(),
        categories: categories_json,
        tags: serde_json::to_string(&cats)?,
        confidence: "high".into(),
    };

    LearningsRepository::new(pool).insert(&row).await?;

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

/// Outcome of attempting to capture a single atomized section.
enum SectionOutcome {
    Captured { kind: &'static str, id: String },
    Skipped { reason: &'static str },
}

/// Atomizing capture: each `## ` section → its own object, type inferred from the
/// filename (Decisions→decision, Learnings→learning, People→person, else note).
/// A section whose guard yields a `rejected` (credential-class) sighting is SKIPPED
/// with a warning; all other sections still get captured.
async fn run_atomize(
    pool: &SqlitePool,
    args: &CaptureArgs,
    _raw: &str,
    sections: Vec<altevra_vault::Section>,
    domain: Domain,
    declared: Sensitivity,
) -> anyhow::Result<()> {
    let kind = infer_kind(&args.file);
    let stem = slugify(&file_stem(&args.file));
    let learnings = LearningsRepository::new(pool);

    let mut outcomes: Vec<SectionOutcome> = Vec::new();
    let mut captured = 0usize;
    let mut skipped_secret = 0usize;
    let mut total_redactions = 0usize;
    let mut conformant_n = 0usize;
    let mut needs_structure_n = 0usize;

    for sec in &sections {
        // ---- per-section safety gate ----
        let guarded = guard_text(&sec.body, declared.clone());

        // Credential-class sighting → skip THIS section, keep going (never store).
        if guarded.sightings.iter().any(|s| s.action == "rejected") {
            skipped_secret += 1;
            eprintln!(
                "  ⚠ skipped section '{}' — credential-class secret detected (not stored)",
                sec.heading
            );
            outcomes.push(SectionOutcome::Skipped {
                reason: "credential",
            });
            continue;
        }
        total_redactions += guarded.sightings.len();

        let mut sensitivity = guarded.sensitivity.clone();
        if domain.is_high_water() {
            sensitivity = sensitivity.combine(&Sensitivity::Restricted);
        }

        // Tags/categories: domain + a `kind:<type>` tag so atomized objects are
        // filterable by their inferred type (TAG-1 already satisfied by domain).
        let kind_tag = format!("kind:{kind}");
        let mut cats = build_categories(&domain, &args.categories, Some(&kind_tag));

        // Section-template conformance (Phase 1): tag the object so recall can
        // surface "this note needs cleanup". `conformant` vs `needs-structure`.
        let conf = section_conformance(sec, kind);
        let conf_tag = if conf.conformant {
            "conformant"
        } else {
            "needs-structure"
        };
        if !cats.iter().any(|c| c == conf_tag) {
            cats.push(conf_tag.to_string());
        }
        if conf.conformant {
            conformant_n += 1;
        } else {
            needs_structure_n += 1;
        }

        // Stable id: capture-<stem>-<section-slug>-<8charhash-of-section-body>.
        let id = format!(
            "capture-{}-{}-{}",
            stem,
            short_section_slug(&sec.heading),
            short_hash(&sec.body)
        );

        let row = LearningRow {
            id: id.clone(),
            title: sec.heading.clone(),
            body: guarded.value.clone(),
            status: "active".into(),
            domain: domain.to_string(),
            scope: None,
            sensitivity: sensitivity.to_string(),
            provenance: provenance_json(&args.file, sec.date.map(|d| d.to_string()))?,
            redaction_status: guarded.redaction_status.to_string(),
            categories: serde_json::to_string(&cats)?,
            tags: serde_json::to_string(&cats)?,
            confidence: "high".into(),
        };
        learnings.insert(&row).await?;
        captured += 1;
        outcomes.push(SectionOutcome::Captured { kind, id });
    }

    if args.json {
        let results: Vec<_> = outcomes
            .iter()
            .map(|o| match o {
                SectionOutcome::Captured { kind, id } => {
                    serde_json::json!({"status": "captured", "kind": kind, "id": id})
                }
                SectionOutcome::Skipped { reason } => {
                    serde_json::json!({"status": "skipped", "reason": reason})
                }
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "file": args.file.display().to_string(),
                "atomized": true,
                "kind": kind,
                "domain": domain.to_string(),
                "sections_found": sections.len(),
                "captured": captured,
                "skipped_credential": skipped_secret,
                "redactions": total_redactions,
                "conformant": conformant_n,
                "needs_structure": needs_structure_n,
                "results": results,
            }))?
        );
    } else {
        println!(
            "Atomized {} → {captured} {kind}(s) captured, {skipped_secret} skipped (credential)",
            args.file.display()
        );
        println!("  domain={domain}  sections_found={}", sections.len());
        println!(
            "  conformance: {conformant_n} conformant, {needs_structure_n} need-structure \
             (tagged for cleanup)"
        );
        if total_redactions > 0 {
            println!("  ⚠ {total_redactions} secret/PII redaction(s) applied before storage");
        }
        println!("  → searchable now: altevra recall \"<term>\"");
    }
    Ok(())
}

/// Filename → atomized object type. Decisions→decision, Learnings→learning,
/// People→person, anything else→note. Matches the R13 template object types.
pub(crate) fn infer_kind(path: &Path) -> &'static str {
    let stem = file_stem(path).to_lowercase();
    if stem.starts_with("decision") {
        "decision"
    } else if stem.starts_with("learning") {
        "learning"
    } else if stem.starts_with("people") || stem.starts_with("person") {
        "person"
    } else {
        "note"
    }
}

/// `true` if the file is a known living aggregate that should auto-atomize (lives
/// under `Memory/` or is a recognized aggregate filename). Conservative: only
/// trips auto-atomize when the path clearly is one of Pavle's canonical aggregates.
pub(crate) fn should_auto_atomize(path: &Path) -> bool {
    let p = path.to_string_lossy().to_lowercase();
    let stem = file_stem(path).to_lowercase();
    let known_aggregate = matches!(
        stem.as_str(),
        "decisions" | "learnings" | "people" | "person"
    );
    let under_memory =
        p.contains("/memory/") || p.contains("\\memory\\") || p.contains("/imperium/memory");
    known_aggregate || under_memory
}

/// Build the category/tag list: always seed the domain (TAG-1), append any
/// `--categories`, and (for atomize) an extra `kind:<type>` tag.
fn build_categories(domain: &Domain, extra: &[String], kind_tag: Option<&str>) -> Vec<String> {
    let mut cats: Vec<String> = vec![domain.to_string()];
    for c in extra {
        let c = c.trim();
        if !c.is_empty() && !cats.iter().any(|x| x == c) {
            cats.push(c.to_string());
        }
    }
    if let Some(k) = kind_tag {
        if !cats.iter().any(|x| x == k) {
            cats.push(k.to_string());
        }
    }
    cats
}

/// Provenance JSON: origin + imported_from path + optional captured-date (the
/// section's heading date when atomizing).
fn provenance_json(file: &Path, created: Option<String>) -> anyhow::Result<String> {
    let mut obj = serde_json::json!({
        "origin": "pavle_direct",
        "imported_from": file.display().to_string(),
    });
    if let Some(c) = created {
        obj["created"] = serde_json::Value::String(c);
    }
    Ok(serde_json::to_string(&obj)?)
}

/// A short, stable slug for a section heading id: slugify, drop leading
/// date-shaped tokens (so the slug carries the topic, not the date — the date
/// lives in provenance), keep the first ~6 words so the id stays bounded.
fn short_section_slug(heading: &str) -> String {
    let slug = slugify(heading);
    // `slugify` turns "2026-06-02 — ReVesta validated" into
    // "2026-06-02-revesta-validated". Drop leading all-numeric tokens (the date).
    let words: Vec<&str> = slug
        .split('-')
        .filter(|w| !w.is_empty())
        .skip_while(|w| w.chars().all(|c| c.is_ascii_digit()))
        .take(6)
        .collect();
    let joined = words.join("-");
    if joined.is_empty() {
        "section".to_string()
    } else {
        joined
    }
}

/// Infer a domain from the note's path — keyless auto-categorization (§3.2). The
/// full LLM classifier is a later upgrade; this covers the Imperium vault layout.
pub(crate) fn infer_domain(path: &Path) -> Domain {
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
            atomize: false,
            no_atomize: false,
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

    #[test]
    fn infer_kind_from_filename() {
        assert_eq!(infer_kind(Path::new("/x/Memory/Decisions.md")), "decision");
        assert_eq!(infer_kind(Path::new("/x/Memory/Learnings.md")), "learning");
        assert_eq!(infer_kind(Path::new("/x/Memory/People.md")), "person");
        assert_eq!(infer_kind(Path::new("/x/Daily/2026-06-02.md")), "note");
    }

    #[test]
    fn auto_atomize_only_for_known_aggregates() {
        assert!(should_auto_atomize(Path::new(
            "/home/pavle/Obsidian/Imperium/Memory/Decisions.md"
        )));
        assert!(should_auto_atomize(Path::new("/x/y/People.md")));
        // a random project note is NOT auto-atomized
        assert!(!should_auto_atomize(Path::new(
            "/x/Projects/altevra/README.md"
        )));
    }

    #[test]
    fn section_slug_drops_leading_date() {
        assert_eq!(
            short_section_slug("2026-06-02 — ReVesta validated hypothesis"),
            "revesta-validated-hypothesis"
        );
        assert_eq!(short_section_slug("Split agent lanes"), "split-agent-lanes");
        // pure-date heading falls back gracefully
        assert_eq!(short_section_slug("2026-06-02"), "section");
    }

    /// Integration test: a Decisions.md-shaped file with 3 `## ` sections (one
    /// carrying a fake `sk-live…` secret) captured via --atomize into a temp DB.
    /// Asserts: 3 object_index rows (the sk-live key is REDACTED, not rejected, so
    /// all 3 store), the secret is gone from the stored body, and recall/search
    /// finds an individual section by its unique words.
    #[tokio::test]
    async fn atomize_captures_three_sections_redacts_secret_and_is_recallable() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("atom.db");
        // Mirror the real Memory/ layout so domain inference + auto-atomize fire.
        let mem = tmp.path().join("Memory");
        std::fs::create_dir_all(&mem).unwrap();
        let file = mem.join("Decisions.md");
        let fake = concat!("sk-", "live", "ABCDEFGHIJKLMNOPQRSTUVWX0123");
        std::fs::write(
            &file,
            format!(
                "# Decisions\n\
                 \n\
                 Preamble prose that is not a section.\n\
                 \n\
                 ## 2026-06-02 — ReVesta validated under twenty numbers\n\
                 \n\
                 We will keep pushing direct-call discovery for surplus buyers.\n\
                 \n\
                 ## Split agent lanes between build and gtm\n\
                 \n\
                 The build agent and the gtm agent stay separate to avoid mixing.\n\
                 \n\
                 ## Vendor onboarding key rotation\n\
                 \n\
                 We stored a key={fake} that must be scrubbed before persistence.\n"
            ),
        )
        .unwrap();

        // No --atomize flag: it auto-atomizes because the file is Memory/Decisions.md
        // with ≥2 sections.
        run(CaptureArgs {
            file: file.clone(),
            domain: None, // inferred: Memory/Decisions.md → business
            sensitivity: "internal".into(),
            title: None,
            categories: vec![],
            atomize: false,
            no_atomize: false,
            db: db.clone(),
            json: true,
        })
        .await
        .unwrap();

        let pool = create_pool(&db.to_string_lossy()).await.unwrap();
        let idx = altevra_db::ObjectIndexRepository::new(&pool);
        let cands = idx.candidates(None).await.unwrap();
        assert_eq!(
            cands.len(),
            3,
            "three sections atomized into three objects (sk-live is redacted, not rejected)"
        );

        // Each object is typed `decision` (filename Decisions.md) and tagged kind:decision.
        let learnings = altevra_db::LearningsRepository::new(&pool);
        for c in &cands {
            let got = learnings.get(&c.id).await.unwrap().unwrap();
            assert!(
                got.tags.contains("kind:decision"),
                "atomized object carries its inferred type tag: {}",
                got.tags
            );
            // Phase 1: every atomized object carries a conformance tag. These three
            // fixture sections are free prose (no **Odluka:**) → needs-structure.
            assert!(
                got.tags.contains("needs-structure"),
                "free-prose decision sections are tagged for cleanup: {}",
                got.tags
            );
            assert!(
                !got.body.contains("sk-live"),
                "the credential must be redacted in EVERY stored section body"
            );
        }

        // recall/search finds ONE individual section by its unique words.
        let fts = altevra_db::FtsRepository::new(&pool);
        let hits = fts.search_objects("twenty numbers", 10).await.unwrap();
        assert_eq!(
            hits.len(),
            1,
            "the unique phrase resolves to exactly one section"
        );
        assert_eq!(
            hits[0].title,
            "2026-06-02 — ReVesta validated under twenty numbers"
        );
        assert!(hits[0].body.contains("surplus buyers"));
    }

    #[tokio::test]
    async fn no_atomize_forces_whole_file_even_for_aggregate() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("whole.db");
        let mem = tmp.path().join("Memory");
        std::fs::create_dir_all(&mem).unwrap();
        let file = mem.join("Decisions.md");
        std::fs::write(
            &file,
            "# Decisions\n\n## One\nalpha body\n\n## Two\nbeta body\n",
        )
        .unwrap();

        run(CaptureArgs {
            file: file.clone(),
            domain: None,
            sensitivity: "internal".into(),
            title: None,
            categories: vec![],
            atomize: false,
            no_atomize: true, // force legacy whole-file
            db: db.clone(),
            json: true,
        })
        .await
        .unwrap();

        let pool = create_pool(&db.to_string_lossy()).await.unwrap();
        let cands = altevra_db::ObjectIndexRepository::new(&pool)
            .candidates(None)
            .await
            .unwrap();
        assert_eq!(
            cands.len(),
            1,
            "--no-atomize stores the whole file as one object"
        );
    }

    #[tokio::test]
    async fn atomize_tags_conformant_section_as_conformant() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("conf.db");
        let mem = tmp.path().join("Memory");
        std::fs::create_dir_all(&mem).unwrap();
        let file = mem.join("Decisions.md");
        // One section in Pavle's real decision shape (Odluka + Zašto) → conformant;
        // one free-prose section → needs-structure.
        std::fs::write(
            &file,
            "# Decisions\n\n\
             ## Conformant lane split\n\
             **Odluka:** Razdvojiti build i gtm agente.\n\n\
             **Zašto:** Sprečava context mixing.\n\n\
             ## Loose note about something\n\
             Samo slobodan tekst bez ijednog labela ovde.\n",
        )
        .unwrap();

        run(CaptureArgs {
            file: file.clone(),
            domain: None,
            sensitivity: "internal".into(),
            title: None,
            categories: vec![],
            atomize: true,
            no_atomize: false,
            db: db.clone(),
            json: true,
        })
        .await
        .unwrap();

        let pool = create_pool(&db.to_string_lossy()).await.unwrap();
        let learnings = altevra_db::LearningsRepository::new(&pool);
        let cands = altevra_db::ObjectIndexRepository::new(&pool)
            .candidates(None)
            .await
            .unwrap();
        assert_eq!(cands.len(), 2);
        let mut saw_conformant = false;
        let mut saw_needs = false;
        for c in &cands {
            let got = learnings.get(&c.id).await.unwrap().unwrap();
            if got.title.contains("Conformant lane split") {
                assert!(
                    got.tags.contains("conformant") && !got.tags.contains("needs-structure"),
                    "Odluka+Zašto section is conformant: {}",
                    got.tags
                );
                saw_conformant = true;
            }
            if got.title.contains("Loose note") {
                assert!(got.tags.contains("needs-structure"), "{}", got.tags);
                saw_needs = true;
            }
        }
        assert!(
            saw_conformant && saw_needs,
            "both conformance states present"
        );
    }
}

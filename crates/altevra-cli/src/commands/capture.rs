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
    /// Optional only with `--watch` (which captures watched dirs continuously).
    pub file: Option<PathBuf>,
    /// WATCH: continuously auto-atomize living docs on save. Watches `--path`
    /// dirs (default ~/Obsidian/Imperium/Memory + Daily). Runs an initial atomize
    /// pass, then blocks until Ctrl+C. SQLite-only; never writes the vault.
    #[arg(long)]
    pub watch: bool,
    /// Directories to watch (repeatable). Defaults to Memory/ + Daily/ when omitted.
    #[arg(long = "path")]
    pub paths: Vec<PathBuf>,
    /// Debounce window (ms) to coalesce editor save bursts in `--watch`.
    #[arg(long, default_value_t = 2_000)]
    pub debounce_ms: u64,
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
    let declared: Sensitivity = args
        .sensitivity
        .parse()
        .map_err(|_| anyhow::anyhow!("unknown --sensitivity '{}'", args.sensitivity))?;

    // --- watch mode: continuous auto-atomize of living docs (SQLite-only) ---
    if args.watch {
        return run_capture_watch(&args, declared).await;
    }

    let file = args
        .file
        .clone()
        .ok_or_else(|| anyhow::anyhow!("a <file> is required (or use --watch)"))?;
    let raw = std::fs::read_to_string(&file)
        .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", file.display()))?;
    if raw.trim().is_empty() {
        anyhow::bail!("{} is empty — nothing to capture", file.display());
    }

    // Domain: explicit flag wins; else infer from the path.
    let domain: Domain = match args.domain.as_deref() {
        Some(d) => d
            .parse()
            .map_err(|_| anyhow::anyhow!("unknown --domain '{d}'"))?,
        None => infer_domain(&file),
    };

    let pool = create_pool(&args.db.to_string_lossy()).await?;
    run_migrations(&pool).await?;

    // Decide the path: explicit --atomize, or auto when the file is a known
    // aggregate (Memory/Decisions|Learnings|People|…) with ≥2 `## ` sections.
    // --no-atomize always forces the legacy whole-file path (back-compat).
    let sections = parse_sections(&raw);
    let want_atomize =
        !args.no_atomize && (args.atomize || (should_auto_atomize(&file) && sections.len() >= 2));

    if want_atomize {
        return run_atomize(&pool, &args, &file, &raw, sections, domain, declared).await;
    }
    run_whole_file(&pool, &args, &file, &raw, domain, declared).await
}

/// `--watch` driver: build the watcher config + block on the watch loop, printing
/// each cycle's atomize summary to stderr (Ctrl+C to stop).
async fn run_capture_watch(args: &CaptureArgs, declared: Sensitivity) -> anyhow::Result<()> {
    let paths = if args.paths.is_empty() {
        super::capture_watch::default_watch_dirs()
    } else {
        args.paths.clone()
    };
    let cfg = super::capture_watch::CaptureWatchConfig {
        paths: paths.clone(),
        debounce_ms: args.debounce_ms,
        declared,
        categories: args.categories.clone(),
        db: args.db.clone(),
    };
    eprintln!(
        "📡 Auto-atomize watching {} dir(s) [debounce {}ms] → {}. Ctrl+C to stop.",
        paths.len(),
        args.debounce_ms,
        args.db.display()
    );
    for p in &paths {
        eprintln!("   • {}", p.display());
    }
    super::capture_watch::run_watch(cfg, |line| eprintln!("{line}")).await
}

/// Legacy whole-file capture: one note → one `learning` (unchanged behaviour).
async fn run_whole_file(
    pool: &SqlitePool,
    args: &CaptureArgs,
    file: &Path,
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
            file.display()
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
        .unwrap_or_else(|| file_stem(file));

    // Tags: always include the domain so TAG-1 holds; merge any --categories.
    let cats = build_categories(&domain, &args.categories, None);
    let categories_json = serde_json::to_string(&cats)?;

    // Deterministic-ish id: capture-<stem>-<8-char content hash> so re-capturing
    // an edited note makes a new row, but the same note twice collides predictably.
    let id = format!("capture-{}-{}", slugify(&file_stem(file)), short_hash(raw));

    let row = LearningRow {
        id: id.clone(),
        title: title.clone(),
        body: guarded.value.clone(),
        status: "active".into(),
        domain: domain.to_string(),
        scope: None,
        sensitivity: sensitivity.to_string(),
        provenance: provenance_json(file, None)?,
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

/// Result of atomizing one file — the counts + outcomes used for the CLI report
/// AND by the `--watch` cycle log. `forgotten` is the count of prior objects from
/// this file that no longer correspond to a current section (incremental cleanup).
pub(crate) struct AtomizeResult {
    pub kind: &'static str,
    pub domain: String,
    pub sections_found: usize,
    pub captured: usize,
    pub skipped_credential: usize,
    pub redactions: usize,
    pub conformant: usize,
    pub needs_structure: usize,
    pub forgotten: usize,
    /// Mention edges (object → person/project) recorded across this file.
    pub mentions_recorded: usize,
    outcomes: Vec<SectionOutcome>,
}

/// Atomize one file into the DB: each `## ` section → its own object (type from
/// filename), credential-class sections skipped, then INCREMENTALLY reconcile —
/// any prior object from this file (same `capture-<stem>-` id prefix) whose section
/// no longer exists or whose content hash changed is `forget`-ten. Idempotent:
/// re-running on unchanged content re-writes identical ids (INSERT OR REPLACE) and
/// forgets nothing. SQLite-only; never touches the vault.
pub(crate) async fn atomize_file(
    pool: &SqlitePool,
    file: &Path,
    sections: &[altevra_vault::Section],
    domain: &Domain,
    declared: &Sensitivity,
    categories: &[String],
    dict: Option<&altevra_core::EntityDictionary>,
) -> anyhow::Result<AtomizeResult> {
    let kind = infer_kind(file);
    let stem = slugify(&file_stem(file));
    let id_prefix = format!("capture-{stem}-");
    let learnings = LearningsRepository::new(pool);
    let idx = altevra_db::ObjectIndexRepository::new(pool);
    let mentions = altevra_db::MentionsRepository::new(pool);

    // Prior objects derived from THIS file (for incremental forget of the stale).
    let prior: Vec<(String, String)> = idx.ids_with_prefix(&id_prefix).await?;

    // Reconcile mention edges too: clear THIS file's prior edges up front, then
    // re-record from the current section set (so a removed/changed mention drops).
    if dict.is_some() {
        mentions.clear_from_prefix(&id_prefix).await?;
    }
    let mut mentions_recorded = 0usize;

    let mut outcomes: Vec<SectionOutcome> = Vec::new();
    let mut current_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut captured = 0usize;
    let mut skipped_secret = 0usize;
    let mut total_redactions = 0usize;
    let mut conformant_n = 0usize;
    let mut needs_structure_n = 0usize;

    for sec in sections {
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
        let mut cats = build_categories(domain, categories, Some(&kind_tag));

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
        // The content hash makes the id change iff the section text changes — the
        // basis of incremental re-atomize.
        let id = format!(
            "capture-{}-{}-{}",
            stem,
            short_section_slug(&sec.heading),
            short_hash(&sec.body)
        );
        current_ids.insert(id.clone());

        let row = LearningRow {
            id: id.clone(),
            title: sec.heading.clone(),
            body: guarded.value.clone(),
            status: "active".into(),
            domain: domain.to_string(),
            scope: None,
            sensitivity: sensitivity.to_string(),
            provenance: provenance_json(file, sec.date.map(|d| d.to_string()))?,
            redaction_status: guarded.redaction_status.to_string(),
            categories: serde_json::to_string(&cats)?,
            tags: serde_json::to_string(&cats)?,
            confidence: "high".into(),
        };
        learnings.insert(&row).await?;
        captured += 1;

        // ---- mention graph: link this object to known people/projects ----
        // Scan the GUARDED (redacted) body so we never index around a secret.
        // Edges are idempotent; high-water domain doesn't change linkage (the
        // edge is local SQLite — SI-7 unaffected).
        if let Some(d) = dict {
            for entity_id in altevra_core::mentioned_entity_ids(&guarded.value, d) {
                let to_type = d
                    .get(&entity_id)
                    .map(|e| e.kind.as_str())
                    .unwrap_or("person");
                if mentions
                    .record("learning", &id, to_type, &entity_id)
                    .await?
                {
                    mentions_recorded += 1;
                }
            }
        }

        outcomes.push(SectionOutcome::Captured { kind, id });
    }

    // ---- incremental reconcile: forget prior objects no longer present ----
    // (section deleted, or its text changed → new hash → new id → old id stale).
    let mut forgotten = 0usize;
    for (otype, oid) in &prior {
        if !current_ids.contains(oid) && idx.forget(otype, oid).await? {
            forgotten += 1;
        }
    }

    Ok(AtomizeResult {
        kind,
        domain: domain.to_string(),
        sections_found: sections.len(),
        captured,
        skipped_credential: skipped_secret,
        redactions: total_redactions,
        conformant: conformant_n,
        needs_structure: needs_structure_n,
        forgotten,
        mentions_recorded,
        outcomes,
    })
}

/// One-shot atomizing capture (the CLI report layer over `atomize_file`).
async fn run_atomize(
    pool: &SqlitePool,
    args: &CaptureArgs,
    file: &Path,
    _raw: &str,
    sections: Vec<altevra_vault::Section>,
    domain: Domain,
    declared: Sensitivity,
) -> anyhow::Result<()> {
    // Build the known-entity dictionary (People.md + project registry + mentors)
    // so each atomized section is cross-linked to the people/projects it mentions.
    let dict = super::entity_dict::build_dictionary(file, None);
    let res = atomize_file(
        pool,
        file,
        &sections,
        &domain,
        &declared,
        &args.categories,
        Some(&dict),
    )
    .await?;

    if args.json {
        let results: Vec<_> = res
            .outcomes
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
                "file": file.display().to_string(),
                "atomized": true,
                "kind": res.kind,
                "domain": res.domain,
                "sections_found": res.sections_found,
                "captured": res.captured,
                "skipped_credential": res.skipped_credential,
                "redactions": res.redactions,
                "conformant": res.conformant,
                "needs_structure": res.needs_structure,
                "forgotten": res.forgotten,
                "mentions_recorded": res.mentions_recorded,
                "results": results,
            }))?
        );
    } else {
        println!(
            "Atomized {} → {} {}(s) captured, {} skipped (credential), {} forgotten (stale)",
            file.display(),
            res.captured,
            res.kind,
            res.skipped_credential,
            res.forgotten
        );
        println!(
            "  domain={}  sections_found={}",
            res.domain, res.sections_found
        );
        println!(
            "  conformance: {} conformant, {} need-structure (tagged for cleanup)",
            res.conformant, res.needs_structure
        );
        if res.mentions_recorded > 0 {
            println!(
                "  mention graph: {} edge(s) to known people/projects",
                res.mentions_recorded
            );
        }
        if res.redactions > 0 {
            println!(
                "  ⚠ {} secret/PII redaction(s) applied before storage",
                res.redactions
            );
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
            file: Some(note),
            watch: false,
            paths: vec![],
            debounce_ms: 2000,
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
            file: Some(file.clone()),
            watch: false,
            paths: vec![],
            debounce_ms: 2000,
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
            file: Some(file.clone()),
            watch: false,
            paths: vec![],
            debounce_ms: 2000,
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
            file: Some(file.clone()),
            watch: false,
            paths: vec![],
            debounce_ms: 2000,
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

    /// The headline incremental-idempotency contract: re-atomizing an EDITED living
    /// doc reflects EXACTLY the new section set — unchanged sections keep their id,
    /// a changed section updates, a removed section is forgotten, a new section is
    /// added. No duplicates.
    #[tokio::test]
    async fn incremental_reatomize_reflects_exactly_v2() {
        use altevra_db::ObjectIndexRepository;

        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("inc.db");
        let mem = tmp.path().join("Memory");
        std::fs::create_dir_all(&mem).unwrap();
        let file = mem.join("Decisions.md");
        let pool = create_pool(&db.to_string_lossy()).await.unwrap();
        run_migrations(&pool).await.unwrap();

        let declared: Sensitivity = "internal".parse().unwrap();
        let domain = infer_domain(&file);

        // --- v1: 3 sections ---
        let v1 = "# Decisions\n\n\
                  ## Section one stable\nThe first decision body stays the same.\n\n\
                  ## Section two will change\nOriginal text for section two.\n\n\
                  ## Section three will be removed\nThis section disappears in v2.\n";
        std::fs::write(&file, v1).unwrap();
        let secs1 = parse_sections(&std::fs::read_to_string(&file).unwrap());
        let r1 = atomize_file(&pool, &file, &secs1, &domain, &declared, &[], None)
            .await
            .unwrap();
        assert_eq!(r1.captured, 3);
        assert_eq!(r1.forgotten, 0);

        // Helper: live (non-forgotten) object ids currently in the index.
        async fn live_ids(pool: &SqlitePool) -> std::collections::HashSet<String> {
            ObjectIndexRepository::new(pool)
                .candidates(None)
                .await
                .unwrap()
                .into_iter()
                .filter(|c| c.status != "forgotten")
                .map(|c| c.id)
                .collect()
        }
        let v1_ids = live_ids(&pool).await;
        assert_eq!(v1_ids.len(), 3, "v1 → 3 live objects");

        // Capture the id of the stable section (its hash must NOT change in v2).
        let stable_id = format!(
            "capture-decisions-{}-{}",
            short_section_slug("Section one stable"),
            short_hash("The first decision body stays the same.")
        );
        assert!(
            v1_ids.contains(&stable_id),
            "stable section id present in v1"
        );

        // --- v2: section 1 unchanged, section 2 text changed, section 3 removed,
        //         section 4 added ---
        let v2 = "# Decisions\n\n\
                  ## Section one stable\nThe first decision body stays the same.\n\n\
                  ## Section two will change\nCOMPLETELY rewritten text for section two now.\n\n\
                  ## Section four brand new\nA newly added fourth section appears.\n";
        std::fs::write(&file, v2).unwrap();
        let secs2 = parse_sections(&std::fs::read_to_string(&file).unwrap());
        let r2 = atomize_file(&pool, &file, &secs2, &domain, &declared, &[], None)
            .await
            .unwrap();
        assert_eq!(r2.captured, 3, "v2 has 3 sections");
        // Forgotten: section-two-OLD-hash + section-three (removed) = 2.
        assert_eq!(
            r2.forgotten, 2,
            "old section-two + removed section-three forgotten"
        );

        let v2_ids = live_ids(&pool).await;
        assert_eq!(
            v2_ids.len(),
            3,
            "exactly 3 live objects after v2 (no duplicates)"
        );

        // The stable section kept its id (unchanged hash) → still live.
        assert!(
            v2_ids.contains(&stable_id),
            "unchanged section keeps its id"
        );

        // The removed section is gone.
        let removed_id = format!(
            "capture-decisions-{}-{}",
            short_section_slug("Section three will be removed"),
            short_hash("This section disappears in v2.")
        );
        assert!(
            !v2_ids.contains(&removed_id),
            "removed section's object is forgotten"
        );

        // The changed section's OLD id is gone, NEW id is present.
        let s2_old = format!(
            "capture-decisions-{}-{}",
            short_section_slug("Section two will change"),
            short_hash("Original text for section two.")
        );
        let s2_new = format!(
            "capture-decisions-{}-{}",
            short_section_slug("Section two will change"),
            short_hash("COMPLETELY rewritten text for section two now.")
        );
        assert!(
            !v2_ids.contains(&s2_old),
            "stale changed-section id forgotten"
        );
        assert!(
            v2_ids.contains(&s2_new),
            "updated changed-section id present"
        );

        // recall reflects the change: the NEW text is findable, the OLD is not.
        let fts = altevra_db::FtsRepository::new(&pool);
        assert_eq!(
            fts.search_objects("COMPLETELY rewritten", 10)
                .await
                .unwrap()
                .len(),
            1,
            "new section two text is recallable"
        );
        assert!(
            fts.search_objects("disappears in v2", 10)
                .await
                .unwrap()
                .is_empty(),
            "removed section is no longer recallable"
        );
        assert_eq!(
            fts.search_objects("newly added fourth", 10)
                .await
                .unwrap()
                .len(),
            1,
            "the added section is recallable"
        );
    }

    /// Atomizing with a dictionary records mention edges, and re-atomizing an
    /// edited file reconciles them (a dropped mention loses its edge).
    #[tokio::test]
    async fn atomize_records_and_reconciles_mention_edges() {
        use altevra_core::{EntityDictionary, EntityKind};
        use altevra_db::MentionsRepository;

        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("ment.db");
        let mem = tmp.path().join("Memory");
        std::fs::create_dir_all(&mem).unwrap();
        let file = mem.join("Decisions.md");
        let pool = create_pool(&db.to_string_lossy()).await.unwrap();
        run_migrations(&pool).await.unwrap();
        let declared: Sensitivity = "internal".parse().unwrap();
        let domain = infer_domain(&file);

        let mut dict = EntityDictionary::new();
        dict.add_person("djordje", "Đorđe Dimitrijević", &["Đorđe".into()]);
        dict.add_project("revesta", "ReVesta", &["Simple Surplus".into()]);
        assert_eq!(dict.people[0].kind, EntityKind::Person);

        // v1: section mentions BOTH Đorđe and ReVesta.
        std::fs::write(
            &file,
            "# Decisions\n\n## Lane split\nĐorđe je rekao da ReVesta ostaje P0 prioritet.\n",
        )
        .unwrap();
        let secs = parse_sections(&std::fs::read_to_string(&file).unwrap());
        let r = atomize_file(&pool, &file, &secs, &domain, &declared, &[], Some(&dict))
            .await
            .unwrap();
        assert_eq!(r.mentions_recorded, 2, "Đorđe + ReVesta linked");

        let ment = MentionsRepository::new(&pool);
        assert_eq!(
            ment.objects_mentioning("person:djordje", 10)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            ment.objects_mentioning("project:revesta", 10)
                .await
                .unwrap()
                .len(),
            1
        );

        // v2: edit the section to DROP Đorđe (only ReVesta now). Re-atomize must
        // reconcile: the Đorđe edge disappears, ReVesta remains.
        std::fs::write(
            &file,
            "# Decisions\n\n## Lane split\nReVesta ostaje P0 prioritet, fokus na prodaju.\n",
        )
        .unwrap();
        let secs2 = parse_sections(&std::fs::read_to_string(&file).unwrap());
        atomize_file(&pool, &file, &secs2, &domain, &declared, &[], Some(&dict))
            .await
            .unwrap();
        assert!(
            ment.objects_mentioning("person:djordje", 10)
                .await
                .unwrap()
                .is_empty(),
            "dropped mention → edge reconciled away"
        );
        assert_eq!(
            ment.objects_mentioning("project:revesta", 10)
                .await
                .unwrap()
                .len(),
            1,
            "still-present mention keeps its edge"
        );
    }
}

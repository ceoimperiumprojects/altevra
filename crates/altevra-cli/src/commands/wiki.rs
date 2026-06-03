//! `altevra wiki` — list / show / search the living wiki layer.
//!
//! Phase 1 (Resident + Wiki foundation): reads pages from disk via
//! `altevra-vault::wiki`. The SQLite index (migration 018) is populated
//! lazily on `list` so callers don't have to manually sync. Phase 5 will
//! wire Wiki Curator to keep the index fresh automatically.

use altevra_core::security::Sensitivity;
use altevra_db::{create_pool, run_migrations, WikiPagesRepository};
use altevra_secrets::guard_text;
use altevra_vault::{list_wiki_pages, WikiPage};
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum WikiCommands {
    /// List all wiki pages (alphabetical by topic)
    List(WikiListArgs),
    /// Print a wiki page by topic
    Show(WikiShowArgs),
    /// Search wiki by topic / title substring
    Search(WikiSearchArgs),
}

#[derive(Args)]
pub struct WikiListArgs {
    /// Wiki root (defaults to `wiki/` under cwd)
    #[arg(long, default_value = "wiki")]
    pub root: PathBuf,
    /// Emit JSON instead of human-readable
    #[arg(long)]
    pub json: bool,
    /// Sync to SQLite index while listing (default: true)
    #[arg(long, default_value_t = true)]
    pub sync: bool,
    /// Database path
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
}

#[derive(Args)]
pub struct WikiShowArgs {
    /// Topic to render
    pub topic: String,
    /// Wiki root
    #[arg(long, default_value = "wiki")]
    pub root: PathBuf,
    /// Emit JSON metadata + body instead of raw markdown
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct WikiSearchArgs {
    /// Substring to search across topic and title
    pub query: String,
    #[arg(long, default_value = "wiki")]
    pub root: PathBuf,
    #[arg(long, default_value_t = 20)]
    pub limit: i64,
    #[arg(long)]
    pub json: bool,
}

pub async fn run(cmd: WikiCommands) -> anyhow::Result<()> {
    match cmd {
        WikiCommands::List(a) => run_list(a).await,
        WikiCommands::Show(a) => run_show(a).await,
        WikiCommands::Search(a) => run_search(a).await,
    }
}

async fn run_list(args: WikiListArgs) -> anyhow::Result<()> {
    let pages = list_wiki_pages(&args.root)?;

    if args.sync {
        if let Some(parent) = args.db.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let pool = create_pool(&args.db.to_string_lossy()).await?;
        run_migrations(&pool).await?;
        let repo = WikiPagesRepository::new(&pool);
        for page in &pages {
            let id = sync_one_page(&repo, page).await?;
            repo.replace_links(id, &page.wiki_links).await?;
        }
    }

    if args.json {
        let arr: Vec<_> = pages.iter().map(page_to_json).collect();
        println!("{}", serde_json::to_string_pretty(&arr)?);
    } else {
        println!("Wiki pages ({}):\n", pages.len());
        for p in &pages {
            println!(
                "  {topic:30}  [{status}]  {confidence}  sources={src}  → {path}",
                topic = p.topic,
                status = p.status.as_str(),
                confidence = p.confidence.as_str(),
                src = p.source_count,
                path = p.path.display()
            );
        }
    }
    Ok(())
}

/// Sync one wiki page into SQLite AND route it into the retrieval substrate so it
/// is recallable (FTS + packet candidate), mirroring the capture path's safety
/// contract (`run_whole_file`):
///   * `guard_text` scrubs secrets/PII from the body BEFORE anything is indexed.
///   * a credential-class sighting (PEM / db-url → `action == "rejected"`) is
///     FAIL-CLOSED: the page metadata is still upserted (so the row exists) but it
///     is NEVER indexed — the un-indexable secret body must not enter `object_fts`.
///   * a high-water domain (inferred from the page path) escalates the page's
///     sensitivity to Restricted, same rule as capture / `ingest_guard`.
///
/// An empty body is never indexed (nothing to retrieve) — metadata only.
async fn sync_one_page(
    repo: &WikiPagesRepository<'_>,
    page: &WikiPage,
) -> anyhow::Result<uuid::Uuid> {
    let slug = slugify(&page.topic);
    let path = page.path.to_string_lossy();

    // Domain: high-water-aware inference from the page path (reuses capture's
    // pub(crate) inferrer so wiki paths get the same domain map as notes).
    let domain = super::capture::infer_domain(&page.path);

    // The page's own frontmatter sensitivity is the declared floor; the guard may
    // only RAISE it. Fall back to Internal if the frontmatter value is unknown.
    let declared: Sensitivity = page.sensitivity.parse().unwrap_or(Sensitivity::Internal);

    // ---- the safety gate (caller-guards; upsert_indexed never re-guards) ----
    let guarded = guard_text(&page.body, declared);

    // Fail-closed: a credential-class secret must NEVER be indexed. Upsert the
    // metadata row via the plain (un-indexed) path so the page still exists in
    // `wiki_pages`, warn, and skip indexing entirely.
    if guarded.sightings.iter().any(|s| s.action == "rejected") {
        eprintln!(
            "⚠ wiki page '{}' contains a credential-class secret — metadata synced \
             but NOT indexed (remove it / store via `altevra secrets set`).",
            page.topic
        );
        return repo
            .upsert(
                &page.topic,
                &slug,
                &path,
                page.status.as_str(),
                page.confidence.as_str(),
                &page.sensitivity,
                page.source_count as i64,
                page.last_synthesized_at,
                page.title.as_deref(),
                &page.checksum,
            )
            .await;
    }

    // High-water domain escalation — personal/health/relationship/… → Restricted,
    // so a high-water wiki page can't default-down and leak (SI-7 / R11 parity).
    let mut sensitivity = guarded.sensitivity.clone();
    if domain.is_high_water() {
        sensitivity = sensitivity.combine(&Sensitivity::Restricted);
    }

    // Categories/tags mirror capture's shape: the (high-water-aware) domain first
    // so TAG-1 holds, plus a `wiki` tag, serialized to JSON.
    let cats = vec![domain.to_string(), "wiki".to_string()];
    let cats_json = serde_json::to_string(&cats)?;

    // Never index an empty body — there is nothing to retrieve. Upsert metadata
    // only by passing a non-scanned verdict, which makes upsert_indexed skip the
    // index write (TAG-1: untagged/unindexable content never enters the index).
    let redaction_status = if page.body.trim().is_empty() {
        "unscanned".to_string()
    } else {
        guarded.redaction_status.to_string()
    };

    repo.upsert_indexed(
        &page.topic,
        &slug,
        &path,
        page.status.as_str(),
        page.confidence.as_str(),
        &sensitivity.to_string(),
        page.source_count as i64,
        page.last_synthesized_at,
        page.title.as_deref(),
        &page.checksum,
        &domain.to_string(),
        &cats_json,
        &cats_json,
        &guarded.value,
        &redaction_status,
    )
    .await
}

async fn run_show(args: WikiShowArgs) -> anyhow::Result<()> {
    let page = find_page_by_topic(&args.root, &args.topic)?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "topic": page.topic,
                "id": page.id,
                "status": page.status.as_str(),
                "confidence": page.confidence.as_str(),
                "sensitivity": page.sensitivity,
                "source_count": page.source_count,
                "last_synthesized_at": page.last_synthesized_at,
                "related_projects": page.related_projects,
                "related_pages": page.related_pages,
                "wiki_links": page.wiki_links,
                "title": page.title,
                "path": page.path.display().to_string(),
                "body": page.body,
            }))?
        );
    } else {
        // Render the on-disk markdown verbatim.
        let raw = std::fs::read_to_string(&page.path)?;
        print!("{raw}");
    }
    Ok(())
}

async fn run_search(args: WikiSearchArgs) -> anyhow::Result<()> {
    let pages = list_wiki_pages(&args.root)?;
    let q = args.query.to_lowercase();
    let hits: Vec<&WikiPage> = pages
        .iter()
        .filter(|p| {
            p.topic.to_lowercase().contains(&q)
                || p.title
                    .as_deref()
                    .map(|t| t.to_lowercase().contains(&q))
                    .unwrap_or(false)
                || p.body.to_lowercase().contains(&q)
        })
        .take(args.limit as usize)
        .collect();
    if args.json {
        let arr: Vec<_> = hits.iter().map(|p| page_to_json(p)).collect();
        println!("{}", serde_json::to_string_pretty(&arr)?);
    } else if hits.is_empty() {
        println!("No wiki pages match '{}'", args.query);
    } else {
        println!("Wiki search '{}' — {} hits:\n", args.query, hits.len());
        for p in hits {
            println!(
                "  {topic:30}  [{status}]  → {path}",
                topic = p.topic,
                status = p.status.as_str(),
                path = p.path.display()
            );
        }
    }
    Ok(())
}

fn page_to_json(p: &WikiPage) -> serde_json::Value {
    serde_json::json!({
        "topic": p.topic,
        "id": p.id,
        "status": p.status.as_str(),
        "confidence": p.confidence.as_str(),
        "sensitivity": p.sensitivity,
        "source_count": p.source_count,
        "title": p.title,
        "path": p.path.display().to_string(),
        "wiki_links": p.wiki_links,
        "related_pages": p.related_pages,
    })
}

fn find_page_by_topic(root: &std::path::Path, topic: &str) -> anyhow::Result<WikiPage> {
    let pages = list_wiki_pages(root)?;
    pages
        .into_iter()
        .find(|p| p.topic == topic)
        .ok_or_else(|| anyhow::anyhow!("wiki page '{topic}' not found"))
}

fn slugify(topic: &str) -> String {
    topic
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn seed_root() -> TempDir {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("concepts")).unwrap();
        std::fs::write(
            tmp.path().join("concepts/alpha.md"),
            "---\ntopic: alpha\nstatus: living\nconfidence: high\n---\n# Alpha\n\n[[other]]\n",
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("concepts/beta.md"),
            "---\ntopic: beta\nstatus: draft\n---\n# Beta page\n",
        )
        .unwrap();
        tmp
    }

    #[tokio::test]
    async fn list_returns_seeded_pages() {
        let tmp = seed_root();
        let db = tempfile::Builder::new()
            .prefix("altevra-wiki-test-")
            .suffix(".db")
            .tempfile()
            .unwrap();
        run_list(WikiListArgs {
            root: tmp.path().to_path_buf(),
            json: true,
            sync: true,
            db: db.path().to_path_buf(),
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn show_returns_topic() {
        let tmp = seed_root();
        run_show(WikiShowArgs {
            topic: "alpha".into(),
            root: tmp.path().to_path_buf(),
            json: true,
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn search_filters_by_substring() {
        let tmp = seed_root();
        run_search(WikiSearchArgs {
            query: "alpha".into(),
            root: tmp.path().to_path_buf(),
            limit: 10,
            json: true,
        })
        .await
        .unwrap();
    }

    #[test]
    fn slugify_simple() {
        assert_eq!(slugify("Hello World!"), "hello-world-");
        assert_eq!(slugify("alpha-beta"), "alpha-beta");
    }

    /// FIX 1 regression: `wiki list --sync` must INDEX pages (not just upsert
    /// metadata) so they are recallable. After a sync over a seeded page that
    /// carries a unique marker phrase, FTS resolves that phrase to the page as a
    /// `wiki` object.
    #[tokio::test]
    async fn sync_indexes_pages_for_recall() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("concepts")).unwrap();
        // A unique marker phrase that won't collide with any other test text.
        std::fs::write(
            tmp.path().join("concepts/marker.md"),
            "---\ntopic: marker-topic\nstatus: living\nconfidence: high\n---\n# Marker Page\n\nThe quokka xylophone marker phrase lives here.\n",
        )
        .unwrap();
        let db = tmp.path().join("wiki-recall.db");

        run_list(WikiListArgs {
            root: tmp.path().to_path_buf(),
            json: true,
            sync: true,
            db: db.clone(),
        })
        .await
        .unwrap();

        let pool = create_pool(&db.to_string_lossy()).await.unwrap();
        let fts = altevra_db::FtsRepository::new(&pool);
        let hits = fts
            .search_objects("quokka xylophone marker", 10)
            .await
            .unwrap();
        assert_eq!(hits.len(), 1, "the unique phrase resolves to the wiki page");
        assert_eq!(hits[0].object_type, "wiki", "indexed as a wiki object");
        assert_eq!(hits[0].title, "Marker Page");
        assert!(hits[0].body.contains("quokka xylophone marker"));
    }

    /// FIX 1 fail-closed: a wiki page containing a credential-class secret (PEM /
    /// db-url → `rejected`) must NOT be indexed — its metadata row still upserts,
    /// but it never enters `object_index`/`object_fts`. The secret literal is
    /// assembled via `concat!` so no contiguous credential lives in this source.
    #[tokio::test]
    async fn sync_fail_closed_does_not_index_credential_page() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("concepts")).unwrap();
        // A credentialed db-url is a `rejected`-class secret (value never stored).
        // Split into pieces so no whole credential string appears in source.
        let secret = concat!("postgres://", "u:", "longpasswordvalue123", "@h/db");
        std::fs::write(
            tmp.path().join("concepts/secret.md"),
            format!(
                "---\ntopic: secret-topic\nstatus: living\nconfidence: high\n---\n# Secret Page\n\nConnection string {secret} embedded here.\n"
            ),
        )
        .unwrap();
        // Also seed a clean page so we can prove the index has exactly the clean one.
        std::fs::write(
            tmp.path().join("concepts/clean.md"),
            "---\ntopic: clean-topic\nstatus: living\nconfidence: high\n---\n# Clean Page\n\nordinary recallable prose here.\n",
        )
        .unwrap();
        let db = tmp.path().join("wiki-failclosed.db");

        run_list(WikiListArgs {
            root: tmp.path().to_path_buf(),
            json: true,
            sync: true,
            db: db.clone(),
        })
        .await
        .unwrap();

        let pool = create_pool(&db.to_string_lossy()).await.unwrap();

        // Both pages exist as metadata rows in wiki_pages (fail-closed still upserts).
        let repo = WikiPagesRepository::new(&pool);
        assert!(repo.find_by_topic("secret-topic").await.unwrap().is_some());
        assert!(repo.find_by_topic("clean-topic").await.unwrap().is_some());

        // But only the CLEAN page is indexed — the credential page is excluded.
        let idx = altevra_db::ObjectIndexRepository::new(&pool);
        let indexed = idx.candidates(None).await.unwrap();
        assert_eq!(
            indexed.len(),
            1,
            "exactly one wiki object indexed (the credential page is fail-closed)"
        );

        // The credential value never landed in the FTS substrate.
        let fts = altevra_db::FtsRepository::new(&pool);
        assert!(
            fts.search_objects("longpasswordvalue123", 10)
                .await
                .unwrap()
                .is_empty(),
            "the credential is never indexed"
        );
        // The clean page IS recallable.
        let clean = fts.search_objects("ordinary recallable prose", 10).await.unwrap();
        assert_eq!(clean.len(), 1);
        assert_eq!(clean[0].title, "Clean Page");
    }
}

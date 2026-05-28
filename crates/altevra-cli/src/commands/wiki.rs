//! `altevra wiki` — list / show / search the living wiki layer.
//!
//! Phase 1 (Resident + Wiki foundation): reads pages from disk via
//! `altevra-vault::wiki`. The SQLite index (migration 018) is populated
//! lazily on `list` so callers don't have to manually sync. Phase 5 will
//! wire Wiki Curator to keep the index fresh automatically.

use altevra_db::{create_pool, run_migrations, WikiPagesRepository};
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
            let id = repo
                .upsert(
                    &page.topic,
                    &slugify(&page.topic),
                    &page.path.to_string_lossy(),
                    page.status.as_str(),
                    page.confidence.as_str(),
                    &page.sensitivity,
                    page.source_count as i64,
                    page.last_synthesized_at,
                    page.title.as_deref(),
                    &page.checksum,
                )
                .await?;
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
}

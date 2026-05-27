use altevra_memory::{ingest_file, SearchIndex};
use altevra_vault::scan_vault;
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum MemoryCommands {
    /// Ingest a file into the memory index
    Ingest(MemoryIngestArgs),
    /// Search the indexed memory
    Search(MemorySearchArgs),
    /// Get project context (top docs + tasks summary)
    Context(MemoryContextArgs),
    /// Build a full context packet for an agent
    Packet(MemoryPacketArgs),
}

#[derive(Args)]
pub struct MemoryIngestArgs {
    /// File to ingest
    pub path: PathBuf,
    /// Chunk size (chars)
    #[arg(long, default_value_t = 2000)]
    pub chunk_size: usize,
    /// Output JSON
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct MemorySearchArgs {
    /// Query string
    pub query: String,
    /// Vault root to index (defaults to ".")
    #[arg(long, default_value = ".")]
    pub vault: PathBuf,
    /// Result limit
    #[arg(long, default_value_t = 5)]
    pub limit: usize,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct MemoryContextArgs {
    /// Project slug
    #[arg(long)]
    pub project: Option<String>,
    /// Vault root
    #[arg(long, default_value = ".")]
    pub vault: PathBuf,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct MemoryPacketArgs {
    #[arg(long)]
    pub agent: Option<String>,
    #[arg(long, default_value = ".")]
    pub vault: PathBuf,
    #[arg(long)]
    pub json: bool,
}

pub async fn run(cmd: MemoryCommands) -> anyhow::Result<()> {
    match cmd {
        MemoryCommands::Ingest(args) => run_ingest(args).await,
        MemoryCommands::Search(args) => run_search(args).await,
        MemoryCommands::Context(args) => run_context(args).await,
        MemoryCommands::Packet(args) => run_packet(args).await,
    }
}

async fn run_ingest(args: MemoryIngestArgs) -> anyhow::Result<()> {
    let doc = ingest_file(&args.path, args.chunk_size)?;
    if args.json {
        let out = serde_json::json!({
            "document_id": doc.document_id,
            "source_path": doc.source_path,
            "chunks": doc.chunks.len(),
            "checksum": doc.checksum,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("Ingested: {}", doc.source_path.display());
        println!("  Document ID: {}", doc.document_id);
        println!("  Chunks: {}", doc.chunks.len());
        println!("  Checksum: {}", doc.checksum);
    }
    Ok(())
}

async fn run_search(args: MemorySearchArgs) -> anyhow::Result<()> {
    let files = scan_vault(&args.vault)?;
    let mut index = SearchIndex::new();
    for f in &files {
        if let Ok(doc) = ingest_file(&f.path, 2000) {
            index.add_document(doc);
        }
    }
    let hits = index.search(&args.query, args.limit);

    if args.json {
        let out = serde_json::json!({
            "query": args.query,
            "hits": hits.iter().map(|h| serde_json::json!({
                "chunk_id": h.chunk_id,
                "source": h.source_path,
                "heading": h.heading_path,
                "score": h.score,
                "snippet": h.snippet,
            })).collect::<Vec<_>>(),
            "total_chunks": index.len(),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else if hits.is_empty() {
        println!("No matches for: {}", args.query);
        println!("Indexed chunks: {}", index.len());
    } else {
        println!(
            "Results for: {} ({} chunks indexed)",
            args.query,
            index.len()
        );
        for h in &hits {
            let src = h
                .source_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "(inline)".into());
            println!("\n  [{:.3}] {}", h.score, src);
            if !h.heading_path.is_empty() {
                println!("    {}", h.heading_path.join(" > "));
            }
            println!("    {}", h.snippet);
        }
    }
    Ok(())
}

async fn run_context(args: MemoryContextArgs) -> anyhow::Result<()> {
    let files = scan_vault(&args.vault)?;
    let project_filter = args.project.clone();
    let project = project_filter.as_deref().unwrap_or("(all)");

    let relevant: Vec<_> = files
        .into_iter()
        .filter(|f| {
            if let Some(p) = project_filter.as_deref() {
                f.path.to_string_lossy().contains(p)
            } else {
                true
            }
        })
        .collect();

    if args.json {
        let out = serde_json::json!({
            "project": project,
            "vault_files": relevant.iter().map(|f| serde_json::json!({
                "path": f.path,
                "section": f.section,
                "size_bytes": f.size_bytes,
            })).collect::<Vec<_>>(),
            "count": relevant.len(),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("Context for project: {project}");
        println!("Vault files: {}", relevant.len());
        for f in relevant.iter().take(20) {
            println!(
                "  - {} [{}]",
                f.path.display(),
                f.section.as_deref().unwrap_or("root"),
            );
        }
    }
    Ok(())
}

async fn run_packet(args: MemoryPacketArgs) -> anyhow::Result<()> {
    let files = scan_vault(&args.vault)?;
    let packet = serde_json::json!({
        "agent": args.agent,
        "vault_root": args.vault.canonicalize().unwrap_or(args.vault.clone()),
        "file_count": files.len(),
        "sections": files.iter().filter_map(|f| f.section.clone()).collect::<std::collections::BTreeSet<_>>(),
    });
    if args.json {
        println!("{}", serde_json::to_string_pretty(&packet)?);
    } else {
        println!(
            "Agent packet for: {}",
            args.agent.as_deref().unwrap_or("(any)")
        );
        println!("  Vault files: {}", files.len());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn ingest_runs() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("doc.md");
        std::fs::write(&path, "# Title\n\nSome body text.\n").unwrap();
        run_ingest(MemoryIngestArgs {
            path,
            chunk_size: 200,
            json: true,
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn context_empty_vault_runs() {
        let tmp = TempDir::new().unwrap();
        run_context(MemoryContextArgs {
            project: None,
            vault: tmp.path().to_path_buf(),
            json: true,
        })
        .await
        .unwrap();
    }
}

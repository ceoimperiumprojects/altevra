use altevra_research::{scrape_url, synthesize, ResearchPipeline, SynthesisInput};
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum ResearchCommands {
    /// Run full research pipeline (scrape + synthesize)
    Run(ResearchRunArgs),
    /// Scrape a single URL and print extracted content
    Scrape(ResearchScrapeArgs),
    /// Synthesize already-scraped content from JSON file
    Synthesize(ResearchSynthesizeArgs),
}

#[derive(Args)]
pub struct ResearchRunArgs {
    /// Topic/query string
    pub query: String,
    /// URLs to research
    #[arg(long, num_args = 1.., value_delimiter = ',')]
    pub urls: Vec<String>,
    /// Vault root to save into (defaults to no save)
    #[arg(long)]
    pub vault: Option<PathBuf>,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct ResearchScrapeArgs {
    /// URL to scrape
    pub url: String,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct ResearchSynthesizeArgs {
    /// Topic
    pub query: String,
    /// JSON file containing a list of ScrapedPage entries
    #[arg(long)]
    pub pages_file: PathBuf,
    #[arg(long)]
    pub json: bool,
}

pub async fn run(cmd: ResearchCommands) -> anyhow::Result<()> {
    match cmd {
        ResearchCommands::Run(args) => run_full(args).await,
        ResearchCommands::Scrape(args) => run_scrape(args).await,
        ResearchCommands::Synthesize(args) => run_synthesize(args).await,
    }
}

async fn run_full(args: ResearchRunArgs) -> anyhow::Result<()> {
    let pipeline = ResearchPipeline::new(args.vault);
    let result = pipeline.run(&args.query, &args.urls).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!("Research: {}", result.query);
        println!("  Pages scraped: {}", result.pages.len());
        if let Some(p) = &result.saved_path {
            println!("  Saved to: {}", p.display());
        }
        println!("\n{}", result.synthesis);
    }
    Ok(())
}

async fn run_scrape(args: ResearchScrapeArgs) -> anyhow::Result<()> {
    let page = scrape_url(&args.url).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&page)?);
    } else {
        println!("URL: {}", page.url);
        println!("Status: {}", page.status);
        if let Some(t) = &page.title {
            println!("Title: {t}");
        }
        println!("\n{}", page.text);
    }
    Ok(())
}

async fn run_synthesize(args: ResearchSynthesizeArgs) -> anyhow::Result<()> {
    let raw = std::fs::read_to_string(&args.pages_file)?;
    let pages: Vec<altevra_research::ScrapedPage> = serde_json::from_str(&raw)?;
    let synthesis = synthesize(SynthesisInput {
        query: &args.query,
        pages: &pages,
    });
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "query": args.query,
                "synthesis": synthesis,
                "pages": pages.len(),
            }))?
        );
    } else {
        println!("{synthesis}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn synthesize_from_empty_pages() {
        let tmp = TempDir::new().unwrap();
        let pages_file = tmp.path().join("pages.json");
        std::fs::write(&pages_file, "[]").unwrap();
        run_synthesize(ResearchSynthesizeArgs {
            query: "test".into(),
            pages_file,
            json: true,
        })
        .await
        .unwrap();
    }
}

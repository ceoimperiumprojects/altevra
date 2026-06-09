use altevra_core::updates::UpdateFeedItem;
use chrono::{Duration, Local, Utc};
use clap::{Args, Subcommand};

#[derive(Subcommand)]
pub enum JournalCommands {
    /// Show today's journal entries
    Today(JournalArgs),
    /// Generate a journal entry from updates over a window
    Generate(JournalGenerateArgs),
}

#[derive(Args)]
pub struct JournalArgs {
    #[arg(long)]
    pub project: Option<String>,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct JournalGenerateArgs {
    /// Time window (e.g. 24h, 7d)
    #[arg(long, default_value = "24h")]
    pub since: String,
    #[arg(long)]
    pub project: Option<String>,
    /// Output path (defaults to stdout)
    #[arg(long)]
    pub out: Option<std::path::PathBuf>,
    #[arg(long)]
    pub json: bool,
}

pub async fn run(cmd: JournalCommands) -> anyhow::Result<()> {
    match cmd {
        JournalCommands::Today(args) => run_today(args).await,
        JournalCommands::Generate(args) => run_generate(args).await,
    }
}

fn parse_window(s: &str) -> chrono::Duration {
    match s {
        "1h" => Duration::hours(1),
        "24h" => Duration::hours(24),
        "7d" => Duration::days(7),
        "30d" => Duration::days(30),
        _ => Duration::hours(24),
    }
}

fn load_updates_since(window: Duration) -> Vec<UpdateFeedItem> {
    let path = altevra_core::home_dir().join(".altevra/events/updates.jsonl");
    if !path.exists() {
        return vec![];
    }
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    let since = Utc::now() - window;
    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<UpdateFeedItem>(l).ok())
        .filter(|u| u.created_at >= since)
        .collect()
}

async fn run_today(args: JournalArgs) -> anyhow::Result<()> {
    let today_start = Local::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_local_timezone(Local)
        .unwrap()
        .with_timezone(&Utc);
    let window = Utc::now() - today_start;
    let mut items = load_updates_since(window);
    if let Some(p) = &args.project {
        items.retain(|u| u.title.contains(p) || u.update_type.contains(p));
    }
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "date": Local::now().date_naive().to_string(),
                "items": items,
                "count": items.len(),
            }))?
        );
    } else {
        println!("Journal — {}", Local::now().date_naive());
        println!("  {} entries today", items.len());
        for u in &items {
            println!("  • [{}] {}", u.importance, u.title);
        }
    }
    Ok(())
}

async fn run_generate(args: JournalGenerateArgs) -> anyhow::Result<()> {
    let window = parse_window(&args.since);
    let mut items = load_updates_since(window);
    if let Some(p) = &args.project {
        items.retain(|u| u.title.contains(p) || u.update_type.contains(p));
    }
    let mut body = String::new();
    body.push_str(&format!(
        "# Journal — {} (last {})\n\n",
        Local::now().date_naive(),
        args.since
    ));
    body.push_str(&format!("{} updates\n\n", items.len()));
    for u in &items {
        body.push_str(&format!(
            "- [{}] **{}** — {}\n",
            u.importance, u.title, u.short_summary
        ));
    }

    if let Some(out) = &args.out {
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(out, &body)?;
        println!("Written to: {}", out.display());
    } else if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({"body": body, "count": items.len()}))?
        );
    } else {
        print!("{body}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_window_known_values() {
        assert_eq!(parse_window("1h"), Duration::hours(1));
        assert_eq!(parse_window("24h"), Duration::hours(24));
        assert_eq!(parse_window("7d"), Duration::days(7));
        assert_eq!(parse_window("invalid"), Duration::hours(24));
    }

    #[tokio::test]
    async fn today_runs_empty() {
        run_today(JournalArgs {
            project: None,
            json: true,
        })
        .await
        .unwrap();
    }
}

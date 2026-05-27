use altevra_core::updates::{Importance, UpdateFeedItem};
use chrono::{Duration, Utc};
use clap::Args;

#[derive(Args)]
pub struct UpdatesArgs {
    /// Filter by project
    #[arg(long)]
    pub project: Option<String>,

    /// Show updates since (e.g. 1h, 24h, 7d, last-session, or ISO timestamp)
    #[arg(long, default_value = "24h")]
    pub since: String,

    /// Filter by agent ID
    #[arg(long)]
    pub agent: Option<String>,

    /// Only show important updates (high or critical)
    #[arg(long)]
    pub important: bool,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

pub async fn run(args: UpdatesArgs) -> anyhow::Result<()> {
    let since = parse_since(&args.since);
    let _importance_min = if args.important {
        Some(Importance::High)
    } else {
        Some(Importance::Low)
    };

    // In MVP without DB: show sample/no updates with instructions
    let updates: Vec<UpdateFeedItem> = load_local_updates(&args.project, since);

    if args.json {
        let output = serde_json::json!({
            "updates": updates,
            "count": updates.len(),
            "query": {
                "project": args.project,
                "since": args.since,
                "important_only": args.important,
            }
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        if updates.is_empty() {
            println!("No updates found.");
            println!("Hint: Connect to Altevra database for live updates.");
            println!("      Set ALTEVRA_DATABASE_URL and run: altevra serve");
        } else {
            println!("Updates (since {}):", args.since);
            for u in &updates {
                let icon = match u.importance {
                    Importance::Critical => "🚨",
                    Importance::High => "🔴",
                    Importance::Medium => "🟡",
                    Importance::Low => "🟢",
                    Importance::Noise => "⚪",
                };
                println!(
                    "{icon} [{}] {} — {}",
                    u.importance, u.title, u.short_summary
                );
            }
        }
    }

    Ok(())
}

fn parse_since(s: &str) -> chrono::DateTime<Utc> {
    let now = Utc::now();
    match s {
        "last-session" | "24h" => now - Duration::hours(24),
        "1h" => now - Duration::hours(1),
        "7d" => now - Duration::days(7),
        other => other.parse().unwrap_or(now - Duration::hours(24)),
    }
}

fn load_local_updates(
    _project: &Option<String>,
    _since: chrono::DateTime<Utc>,
) -> Vec<UpdateFeedItem> {
    // Loads from .altevra/events/*.jsonl in current directory if available
    // Returns empty for MVP without DB
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_since_24h() {
        let t = parse_since("24h");
        let diff = Utc::now() - t;
        assert!(diff.num_hours() >= 23 && diff.num_hours() <= 25);
    }

    #[test]
    fn test_parse_since_1h() {
        let t = parse_since("1h");
        let diff = Utc::now() - t;
        assert!(diff.num_minutes() >= 59 && diff.num_minutes() <= 61);
    }

    #[tokio::test]
    async fn test_updates_json_output() {
        let args = UpdatesArgs {
            project: Some("altevra".to_string()),
            since: "24h".to_string(),
            agent: None,
            important: false,
            json: true,
        };
        run(args).await.unwrap();
    }
}

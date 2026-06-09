use altevra_core::updates::{Importance, UpdateFeedItem};
use chrono::{Duration, Utc};
use clap::{Args, Subcommand};

#[derive(Args, Clone)]
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

    /// Mark all returned updates as read
    #[arg(long)]
    pub mark_read: bool,

    #[command(subcommand)]
    pub subcommand: Option<UpdatesSubcommand>,
}

#[derive(Subcommand, Clone)]
pub enum UpdatesSubcommand {
    /// Mark updates as read up to a given event ID (or all)
    MarkRead(MarkReadArgs),
}

#[derive(Args, Clone)]
pub struct MarkReadArgs {
    /// Actor type marking read (default: agent)
    #[arg(long, default_value = "agent")]
    pub actor_type: String,
    /// Actor ID
    #[arg(long, default_value = "default")]
    pub actor_id: String,
    /// Specific update id to mark (otherwise mark all current)
    #[arg(long)]
    pub up_to: Option<uuid::Uuid>,
}

pub async fn run(args: UpdatesArgs) -> anyhow::Result<()> {
    if let Some(UpdatesSubcommand::MarkRead(mr)) = args.subcommand.clone() {
        return run_mark_read(mr).await;
    }

    let since = parse_since(&args.since);
    let importance_min = if args.important {
        Some(Importance::High)
    } else {
        None
    };

    let updates = load_local_updates(&args.project, since, importance_min.as_ref());

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
    } else if updates.is_empty() {
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

    if args.mark_read && !updates.is_empty() {
        let last = updates.first().map(|u| u.event_id);
        mark_read_local("agent", "default", last)?;
        println!("\nMarked {} updates as read.", updates.len());
    }

    Ok(())
}

async fn run_mark_read(args: MarkReadArgs) -> anyhow::Result<()> {
    mark_read_local(&args.actor_type, &args.actor_id, args.up_to)?;
    println!(
        "Marked read for {}/{} up_to={:?}",
        args.actor_type, args.actor_id, args.up_to
    );
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
    project: &Option<String>,
    since: chrono::DateTime<Utc>,
    importance_min: Option<&Importance>,
) -> Vec<UpdateFeedItem> {
    let path = altevra_core::home_dir().join(".altevra/events/updates.jsonl");
    if !path.exists() {
        return vec![];
    }
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    let mut items: Vec<UpdateFeedItem> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .filter(|item: &UpdateFeedItem| item.created_at >= since)
        .filter(|item: &UpdateFeedItem| {
            project.as_deref().is_none_or(|p| {
                item.update_type.contains(p)
                    || item.title.contains(p)
                    || item.short_summary.contains(p)
            })
        })
        .filter(|item: &UpdateFeedItem| {
            importance_min
                .as_ref()
                .map(|imin| &item.importance >= *imin)
                .unwrap_or(true)
        })
        .collect();
    items.sort_by_key(|i| std::cmp::Reverse(i.created_at));
    items
}

/// Append a single UpdateFeedItem as a JSONL line to the local events file.
pub fn append_local_update(item: &UpdateFeedItem) {
    let path = altevra_core::home_dir().join(".altevra/events/updates.jsonl");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(line) = serde_json::to_string(item) {
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = writeln!(file, "{line}");
        }
    }
}

/// Persist read-state in a local JSON file (DB-less fallback).
pub fn mark_read_local(
    actor_type: &str,
    actor_id: &str,
    last_event: Option<uuid::Uuid>,
) -> anyhow::Result<()> {
    let path = altevra_core::home_dir().join(".altevra/state/read_state.json");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut map: serde_json::Map<String, serde_json::Value> = if path.exists() {
        let raw = std::fs::read_to_string(&path)?;
        serde_json::from_str(&raw).unwrap_or_default()
    } else {
        Default::default()
    };
    let key = format!("{actor_type}::{actor_id}");
    map.insert(
        key,
        serde_json::json!({
            "last_seen_event_id": last_event,
            "last_seen_at": Utc::now(),
        }),
    );
    std::fs::write(path, serde_json::to_string_pretty(&map)?)?;
    Ok(())
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
            mark_read: false,
            subcommand: None,
        };
        run(args).await.unwrap();
    }
}

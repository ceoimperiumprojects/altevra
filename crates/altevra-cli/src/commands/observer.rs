use altevra_core::events::{ActorType, Event, EventStatus, EventType};
use altevra_core::observer::{detect_patterns, writer, Insight};
use altevra_core::security::Sensitivity;
use altevra_core::updates::UpdateFeedItem;
use chrono::{DateTime, Duration, Utc};
use clap::{Args, Subcommand};
use std::path::PathBuf;
use std::str::FromStr;

#[derive(Subcommand)]
pub enum ObserverCommands {
    /// Run all pattern detectors over recent events and print insights.
    Scan(ScanArgs),
    /// List previously-written insight files in <vault>/10-insights/.
    Insights(InsightsArgs),
}

#[derive(Args)]
pub struct ScanArgs {
    /// Time window to consider (e.g. 24h, 7d, 30d).
    #[arg(long, default_value = "7d")]
    pub since: String,

    /// Vault root (defaults to current directory).
    #[arg(long, default_value = ".")]
    pub vault: PathBuf,

    /// Also write `vault/10-insights/auto-YYYYMMDD.md`.
    #[arg(long)]
    pub write: bool,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct InsightsArgs {
    /// Vault root.
    #[arg(long, default_value = ".")]
    pub vault: PathBuf,

    /// Only show the latest insight file (path).
    #[arg(long)]
    pub latest: bool,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

pub async fn run(cmd: ObserverCommands) -> anyhow::Result<()> {
    match cmd {
        ObserverCommands::Scan(args) => run_scan(args).await,
        ObserverCommands::Insights(args) => run_insights(args).await,
    }
}

async fn run_scan(args: ScanArgs) -> anyhow::Result<()> {
    let since = Utc::now() - parse_window(&args.since);
    let events = load_events_for_observer(&args.vault, since);
    let updates = load_updates(&args.vault, since);

    let insights = detect_patterns(&events, &updates);

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "since": args.since,
                "count": insights.len(),
                "insights": insights,
            }))?
        );
    } else if insights.is_empty() {
        println!("No patterns detected in last {}.", args.since);
    } else {
        println!(
            "Observer — {} insight(s) in last {}:\n",
            insights.len(),
            args.since
        );
        for ins in &insights {
            print_insight(ins);
        }
    }

    if args.write {
        let dest = writer::write_insights_markdown(&insights, &args.vault)?;
        println!("\nWritten: {}", dest.display());
    }

    Ok(())
}

async fn run_insights(args: InsightsArgs) -> anyhow::Result<()> {
    let files = writer::list_insight_files(&args.vault)?;
    if args.latest {
        match files.first() {
            Some(p) if args.json => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "latest": p.display().to_string(),
                    }))?
                );
            }
            Some(p) => println!("{}", p.display()),
            None if args.json => println!("{}", serde_json::json!({"latest": null})),
            None => println!("(no insight files)"),
        }
        return Ok(());
    }

    if args.json {
        let arr: Vec<String> = files.iter().map(|p| p.display().to_string()).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "count": arr.len(),
                "files": arr,
            }))?
        );
    } else if files.is_empty() {
        println!("No insight files in {}/10-insights/.", args.vault.display());
    } else {
        for p in &files {
            println!("{}", p.display());
        }
    }
    Ok(())
}

fn print_insight(ins: &Insight) {
    let icon = match ins.importance {
        altevra_core::updates::Importance::Critical => "[CRITICAL]",
        altevra_core::updates::Importance::High => "[HIGH]",
        altevra_core::updates::Importance::Medium => "[MEDIUM]",
        altevra_core::updates::Importance::Low => "[LOW]",
        altevra_core::updates::Importance::Noise => "[NOISE]",
    };
    println!("{icon} {} ({})", ins.title, ins.kind);
    println!("    {}", ins.summary);
    if let Some(a) = &ins.recommended_action {
        println!("    → {a}");
    }
    println!("    evidence: {} item(s)\n", ins.evidence.len());
}

fn parse_window(s: &str) -> Duration {
    match s {
        "1h" => Duration::hours(1),
        "24h" | "1d" => Duration::hours(24),
        "7d" => Duration::days(7),
        "14d" => Duration::days(14),
        "30d" => Duration::days(30),
        other => other
            .strip_suffix('h')
            .and_then(|n| n.parse::<i64>().ok())
            .map(Duration::hours)
            .or_else(|| {
                other
                    .strip_suffix('d')
                    .and_then(|n| n.parse::<i64>().ok())
                    .map(Duration::days)
            })
            .unwrap_or_else(|| Duration::hours(24)),
    }
}

fn load_updates(vault: &std::path::Path, since: DateTime<Utc>) -> Vec<UpdateFeedItem> {
    let path = vault.join(".altevra/events/updates.jsonl");
    if !path.exists() {
        return vec![];
    }
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<UpdateFeedItem>(l).ok())
        .filter(|u| u.created_at >= since)
        .collect()
}

/// Load Events for the observer.
///
/// Prefer a raw `events.jsonl` if present (canonical). If only `updates.jsonl`
/// exists, synthesize lightweight Events from the UpdateFeedItem stream so
/// detectors still have something to chew on. The synthesized events carry
/// `update_type` as a string we map back to EventType where possible.
fn load_events_for_observer(vault: &std::path::Path, since: DateTime<Utc>) -> Vec<Event> {
    let events_path = vault.join(".altevra/events/events.jsonl");
    if events_path.exists() {
        let content = std::fs::read_to_string(&events_path).unwrap_or_default();
        return content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str::<Event>(l).ok())
            .filter(|e| e.created_at >= since)
            .collect();
    }

    // Fallback: derive minimal Events from UpdateFeedItem stream.
    let updates = load_updates(vault, since);
    updates
        .into_iter()
        .filter_map(|u| {
            let et = EventType::from_str(&u.update_type).ok()?;
            Some(Event {
                id: u.event_id,
                event_type: et,
                project_id: u.project_id,
                actor_type: ActorType::System,
                actor_id: None,
                source: u.update_type.clone(),
                entity_type: None,
                entity_id: extract_entity_id_from_affected(&u.affected_entities),
                title: u.title,
                summary: Some(u.short_summary),
                payload: serde_json::Value::Object(Default::default()),
                sensitivity: Sensitivity::Internal,
                created_at: u.created_at,
                processed_at: None,
                status: EventStatus::Processed,
            })
        })
        .collect()
}

fn extract_entity_id_from_affected(v: &serde_json::Value) -> Option<String> {
    v.as_array()
        .and_then(|arr| arr.first())
        .and_then(|first| first.get("id"))
        .and_then(|id| id.as_str())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_window_known_and_dynamic() {
        assert_eq!(parse_window("1h"), Duration::hours(1));
        assert_eq!(parse_window("24h"), Duration::hours(24));
        assert_eq!(parse_window("7d"), Duration::days(7));
        assert_eq!(parse_window("48h"), Duration::hours(48));
        assert_eq!(parse_window("3d"), Duration::days(3));
        assert_eq!(parse_window("garbage"), Duration::hours(24));
    }

    #[tokio::test]
    async fn scan_runs_on_empty_vault() {
        let tmp = tempfile::tempdir().unwrap();
        let args = ScanArgs {
            since: "7d".to_string(),
            vault: tmp.path().to_path_buf(),
            write: false,
            json: true,
        };
        run_scan(args).await.unwrap();
    }

    #[tokio::test]
    async fn scan_with_write_creates_file() {
        let tmp = tempfile::tempdir().unwrap();
        let args = ScanArgs {
            since: "7d".to_string(),
            vault: tmp.path().to_path_buf(),
            write: true,
            json: true,
        };
        run_scan(args).await.unwrap();
        let listed = writer::list_insight_files(tmp.path()).unwrap();
        assert_eq!(listed.len(), 1);
    }

    #[tokio::test]
    async fn insights_list_empty_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let args = InsightsArgs {
            vault: tmp.path().to_path_buf(),
            latest: false,
            json: true,
        };
        run_insights(args).await.unwrap();
    }

    #[test]
    fn load_events_synthesizes_from_updates_when_no_events_file() {
        let tmp = tempfile::tempdir().unwrap();
        let events_dir = tmp.path().join(".altevra/events");
        std::fs::create_dir_all(&events_dir).unwrap();
        let u = UpdateFeedItem {
            id: uuid::Uuid::new_v4(),
            event_id: uuid::Uuid::new_v4(),
            project_id: None,
            update_type: "skill_drift_detected".to_string(),
            importance: altevra_core::updates::Importance::High,
            title: "drift".to_string(),
            short_summary: "drifted".to_string(),
            agent_summary: None,
            affected_entities: serde_json::json!([{"type": "skill", "id": "foo"}]),
            recommended_agent_action: None,
            visible_to_agents: true,
            sensitivity: Sensitivity::Internal,
            created_at: Utc::now(),
        };
        std::fs::write(
            events_dir.join("updates.jsonl"),
            format!("{}\n", serde_json::to_string(&u).unwrap()),
        )
        .unwrap();

        let events = load_events_for_observer(tmp.path(), Utc::now() - Duration::days(30));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EventType::SkillDriftDetected);
        assert_eq!(events[0].entity_id.as_deref(), Some("foo"));
    }
}

//! `altevra session` — manage agent sessions (v0.3.1 omniscient recorder).

use altevra_db::{create_pool, run_migrations, SessionRow, SessionsRepository};
use chrono::Utc;
use clap::{Args, Subcommand};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Subcommand)]
pub enum SessionCommands {
    /// Start a new session and print its id (use as input to `turn record`).
    Start(SessionStartArgs),
    /// End an open session (writes ended_at + optional summary).
    End(SessionEndArgs),
    /// List recent sessions.
    List(SessionListArgs),
    /// Show a single session with its turn count + token aggregates.
    Show(SessionShowArgs),
    /// Replay a session — render its turn stream in a chosen tool format.
    Replay(SessionReplayArgs),
}

#[derive(Args)]
pub struct SessionStartArgs {
    #[arg(long, default_value = "claude-code")]
    pub tool: String,
    #[arg(long)]
    pub project: Option<String>,
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct SessionEndArgs {
    pub session_id: String,
    #[arg(long)]
    pub summary: Option<String>,
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
}

#[derive(Args)]
pub struct SessionListArgs {
    #[arg(long)]
    pub tool: Option<String>,
    #[arg(long)]
    pub project: Option<String>,
    /// Time window — accepts `Nh`, `Nd`, `Nw` (e.g. "7d", "12h", "2w").
    #[arg(long)]
    pub since: Option<String>,
    #[arg(long, default_value_t = 20)]
    pub limit: i64,
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct SessionReplayArgs {
    pub session_id: String,
    /// Target tool format: claude-code | codex | cursor | antigravity | hermes | text.
    #[arg(long, default_value = "text")]
    pub tool: String,
    #[arg(long, default_value_t = 1000)]
    pub turn_limit: i64,
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct SessionShowArgs {
    pub session_id: String,
    #[arg(long, default_value_t = 100)]
    pub turn_limit: i64,
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
    #[arg(long)]
    pub json: bool,
}

pub async fn run(cmd: SessionCommands) -> anyhow::Result<()> {
    match cmd {
        SessionCommands::Start(args) => run_start(args).await,
        SessionCommands::End(args) => run_end(args).await,
        SessionCommands::List(args) => run_list(args).await,
        SessionCommands::Show(args) => run_show(args).await,
        SessionCommands::Replay(args) => run_replay(args).await,
    }
}

/// Parse a since string like "7d", "12h", "2w" -> DateTime cutoff.
pub fn parse_since(s: &str) -> anyhow::Result<chrono::DateTime<Utc>> {
    let s = s.trim();
    if s.is_empty() {
        anyhow::bail!("empty --since value");
    }
    let (num, unit) = s.split_at(s.len().saturating_sub(1));
    let n: i64 = num
        .parse()
        .map_err(|_| anyhow::anyhow!("bad number in --since: {s}"))?;
    let dur = match unit {
        "h" | "H" => chrono::Duration::hours(n),
        "d" | "D" => chrono::Duration::days(n),
        "w" | "W" => chrono::Duration::weeks(n),
        _ => anyhow::bail!("unknown --since unit '{unit}' (use h, d, w)"),
    };
    Ok(Utc::now() - dur)
}

async fn open_pool(path: &std::path::Path) -> anyhow::Result<sqlx::SqlitePool> {
    let s = path.to_string_lossy();
    let pool = create_pool(&s).await?;
    run_migrations(&pool).await?;
    Ok(pool)
}

async fn run_start(args: SessionStartArgs) -> anyhow::Result<()> {
    let pool = open_pool(&args.db).await?;
    let repo = SessionsRepository::new(&pool);
    let row = SessionRow {
        id: Uuid::new_v4(),
        tool: args.tool.clone(),
        project_id: None,
        project_name: args.project.clone(),
        started_at: Utc::now(),
        ended_at: None,
        summary: None,
        tokens_in_total: 0,
        tokens_out_total: 0,
        cost_usd_estimate: 0.0,
        turn_count: 0,
        metadata: serde_json::json!({}),
        external_id: None,
        imported_from: None,
        working_dir: std::env::current_dir()
            .ok()
            .map(|p| p.to_string_lossy().into_owned()),
    };
    repo.start_session(&row).await?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "session_id": row.id,
                "tool": row.tool,
                "project": row.project_name,
                "started_at": row.started_at,
            }))?
        );
    } else {
        println!("{}", row.id);
    }
    Ok(())
}

async fn run_end(args: SessionEndArgs) -> anyhow::Result<()> {
    let pool = open_pool(&args.db).await?;
    let repo = SessionsRepository::new(&pool);
    let id = Uuid::parse_str(&args.session_id)?;
    repo.end_session(id, args.summary.as_deref()).await?;
    println!("Session {id} ended");
    Ok(())
}

async fn run_list(args: SessionListArgs) -> anyhow::Result<()> {
    let pool = open_pool(&args.db).await?;
    let repo = SessionsRepository::new(&pool);
    let since = args.since.as_deref().map(parse_since).transpose()?;
    let sessions = if since.is_some() {
        repo.list_sessions_since(
            args.tool.as_deref(),
            args.project.as_deref(),
            since,
            args.limit,
        )
        .await?
    } else {
        repo.list_sessions(args.tool.as_deref(), args.project.as_deref(), args.limit)
            .await?
    };

    if args.json {
        let json: Vec<_> = sessions
            .iter()
            .map(|s| {
                serde_json::json!({
                    "id": s.id,
                    "tool": s.tool,
                    "project": s.project_name,
                    "started_at": s.started_at,
                    "ended_at": s.ended_at,
                    "turn_count": s.turn_count,
                    "tokens_in": s.tokens_in_total,
                    "tokens_out": s.tokens_out_total,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "sessions": json,
                "count": sessions.len(),
            }))?
        );
    } else if sessions.is_empty() {
        println!("No sessions recorded yet.");
    } else {
        println!("Recent sessions ({}):", sessions.len());
        for s in &sessions {
            let status = if s.ended_at.is_some() {
                "closed"
            } else {
                "open"
            };
            println!(
                "  {} [{}] {} — {} turns | {} in | {} out [{status}]",
                &s.id.to_string()[..8],
                s.tool,
                s.project_name.as_deref().unwrap_or("(no project)"),
                s.turn_count,
                s.tokens_in_total,
                s.tokens_out_total,
            );
        }
    }
    Ok(())
}

async fn run_show(args: SessionShowArgs) -> anyhow::Result<()> {
    let pool = open_pool(&args.db).await?;
    let repo = SessionsRepository::new(&pool);
    let id = Uuid::parse_str(&args.session_id)?;
    let session = repo
        .get_session(id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Session not found: {id}"))?;
    let turns = repo.list_turns(id, args.turn_limit).await?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "session": {
                    "id": session.id,
                    "tool": session.tool,
                    "project": session.project_name,
                    "started_at": session.started_at,
                    "ended_at": session.ended_at,
                    "summary": session.summary,
                    "turn_count": session.turn_count,
                    "tokens_in_total": session.tokens_in_total,
                    "tokens_out_total": session.tokens_out_total,
                },
                "turns": turns.iter().map(|t| serde_json::json!({
                    "idx": t.turn_idx,
                    "role": t.role,
                    "content": t.content,
                    "tool_name": t.tool_name,
                    "model": t.model,
                    "tokens_in": t.tokens_in,
                    "tokens_out": t.tokens_out,
                    "latency_ms": t.latency_ms,
                    "redacted_count": t.redacted_count,
                    "created_at": t.created_at,
                })).collect::<Vec<_>>(),
            }))?
        );
    } else {
        println!("Session {}", session.id);
        println!("  Tool:    {}", session.tool);
        println!(
            "  Project: {}",
            session.project_name.as_deref().unwrap_or("(none)")
        );
        println!("  Started: {}", session.started_at);
        if let Some(e) = session.ended_at {
            println!("  Ended:   {e}");
        }
        println!("  Turns:   {}", session.turn_count);
        println!(
            "  Tokens:  in {} | out {}",
            session.tokens_in_total, session.tokens_out_total
        );
        if let Some(s) = &session.summary {
            println!("  Summary: {s}");
        }
        println!();
        for t in &turns {
            let preview: String = t.content.chars().take(120).collect();
            println!(
                "  [{:03}] {} ({}{}) — {}",
                t.turn_idx,
                t.role,
                t.tool_name.as_deref().unwrap_or("-"),
                if t.redacted_count > 0 {
                    format!(", {} redacted", t.redacted_count)
                } else {
                    "".into()
                },
                preview,
            );
        }
    }
    Ok(())
}

async fn run_replay(args: SessionReplayArgs) -> anyhow::Result<()> {
    let pool = open_pool(&args.db).await?;
    let repo = SessionsRepository::new(&pool);
    let id = Uuid::parse_str(&args.session_id)?;
    let session = repo
        .get_session(id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Session not found: {id}"))?;
    let turns = repo.list_turns(id, args.turn_limit).await?;

    if args.json {
        // JSON envelope replays the canonical TurnRow stream.
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "session_id": session.id,
                "tool": session.tool,
                "replay_as": args.tool,
                "turns": turns.iter().map(|t| serde_json::json!({
                    "idx": t.turn_idx,
                    "role": t.role,
                    "content": t.content,
                    "tool_name": t.tool_name,
                    "tool_calls": t.tool_calls,
                    "model": t.model,
                    "tokens_in": t.tokens_in,
                    "tokens_out": t.tokens_out,
                    "created_at": t.created_at,
                })).collect::<Vec<_>>(),
            }))?
        );
        return Ok(());
    }

    // Text rendering — per-tool prefix style.
    let prefix = |role: &str| match args.tool.as_str() {
        "claude-code" => format!("[{}]", role),
        "codex" => format!(">>> {}", role),
        "cursor" => format!("{}:", role),
        "antigravity" => format!("@{}", role),
        "hermes" => format!("hermes/{}", role),
        _ => role.to_string(),
    };

    println!(
        "Replay session {} (tool={}, replay_as={}, {} turns)\n",
        session.id,
        session.tool,
        args.tool,
        turns.len()
    );
    for t in &turns {
        println!(
            "─── turn {:03} ({}) ──────────────",
            t.turn_idx,
            prefix(&t.role)
        );
        if let Some(name) = &t.tool_name {
            println!("tool: {name}");
        }
        println!("{}", t.content);
        println!();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn parse_since_supports_h_d_w() {
        let now = Utc::now();
        let h = parse_since("12h").unwrap();
        let d = parse_since("7d").unwrap();
        let w = parse_since("2w").unwrap();
        assert!((now - h).num_hours() >= 11);
        assert!((now - d).num_days() >= 6);
        assert!((now - w).num_weeks() >= 1);
        assert!(parse_since("bogus").is_err());
        assert!(parse_since("").is_err());
    }

    #[tokio::test]
    async fn start_then_list_then_end() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("altevra.db");

        run_start(SessionStartArgs {
            tool: "claude-code".into(),
            project: Some("altevra".into()),
            db: db.clone(),
            json: true,
        })
        .await
        .unwrap();

        run_list(SessionListArgs {
            tool: None,
            project: None,
            since: None,
            limit: 10,
            db: db.clone(),
            json: true,
        })
        .await
        .unwrap();

        run_list(SessionListArgs {
            tool: None,
            project: None,
            since: Some("7d".into()),
            limit: 10,
            db: db.clone(),
            json: true,
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn replay_renders_existing_session() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("altevra.db");
        // Create a session via the public CLI path.
        run_start(SessionStartArgs {
            tool: "claude-code".into(),
            project: Some("altevra".into()),
            db: db.clone(),
            json: false,
        })
        .await
        .unwrap();

        let pool = open_pool(&db).await.unwrap();
        let repo = SessionsRepository::new(&pool);
        let sessions = repo.list_sessions(None, None, 1).await.unwrap();
        assert_eq!(sessions.len(), 1);
        let sid = sessions[0].id;

        run_replay(SessionReplayArgs {
            session_id: sid.to_string(),
            tool: "claude-code".into(),
            turn_limit: 100,
            db: db.clone(),
            json: true,
        })
        .await
        .unwrap();
    }
}

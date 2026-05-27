//! Drives the import: discover → per-tool parse → upsert → optional LLM
//! summarize → Obsidian ingest → final report.

use chrono::Utc;
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use uuid::Uuid;

use altevra_db::repositories::sessions::{SessionRow, SessionsRepository, TurnRow};

use crate::commands::analyze::{
    discovery::{discover, DiscoveryReport},
    parsers, ImportedSession,
};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ImportStats {
    pub sessions_imported: u64,
    pub sessions_skipped: u64,
    pub turns_imported: u64,
    pub secrets_captured: u64,
    pub llm_summaries: u64,
    pub vault_docs_scanned: u64,
    pub vault_docs_parsed: u64,
    pub by_tool: std::collections::BTreeMap<String, u64>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct AnalyzeOpts {
    pub dry_run: bool,
    pub no_llm_summary: bool,
    pub limit_per_tool: Option<usize>,
    pub only_tool: Option<String>,
}

pub async fn run_analyze(
    pool: &sqlx::SqlitePool,
    opts: AnalyzeOpts,
) -> anyhow::Result<(DiscoveryReport, ImportStats)> {
    let report = discover();
    let mut stats = ImportStats::default();

    if opts.dry_run {
        return Ok((report, stats));
    }

    let repo = SessionsRepository::new(pool);

    let total = report.total_session_files();
    let pb = if total > 0 {
        let pb = ProgressBar::new(total as u64);
        pb.set_style(
            ProgressStyle::with_template(
                "[{bar:40.red/dim}] {pos}/{len} {wide_msg} ({elapsed_precise})",
            )
            .unwrap()
            .progress_chars("█▓░"),
        );
        pb.enable_steady_tick(Duration::from_millis(200));
        Some(pb)
    } else {
        None
    };

    let process = |tool: &str| -> bool {
        opts.only_tool
            .as_deref()
            .map(|t| t == tool)
            .unwrap_or(true)
    };

    // --- Claude Code ---
    if process("claude-code") {
        for (idx, path) in report.claude_code_files.iter().enumerate() {
            if let Some(limit) = opts.limit_per_tool {
                if idx >= limit {
                    break;
                }
            }
            if let Some(pb) = &pb {
                pb.set_message(format!("claude-code: {}", path.display()));
            }
            match parsers::claude_code::parse_file(path) {
                Ok(Some(session)) => {
                    import_one(&repo, session, &mut stats).await;
                }
                Ok(None) => {}
                Err(e) => stats
                    .errors
                    .push(format!("claude-code {}: {}", path.display(), e)),
            }
            if let Some(pb) = &pb {
                pb.inc(1);
            }
        }
    }

    // --- Cursor ---
    if process("cursor") {
        for (idx, path) in report.cursor_jsonl_files.iter().enumerate() {
            if let Some(limit) = opts.limit_per_tool {
                if idx >= limit {
                    break;
                }
            }
            if let Some(pb) = &pb {
                pb.set_message(format!("cursor: {}", path.display()));
            }
            match parsers::cursor::parse_file(path) {
                Ok(Some(session)) => import_one(&repo, session, &mut stats).await,
                Ok(None) => {}
                Err(e) => stats
                    .errors
                    .push(format!("cursor {}: {}", path.display(), e)),
            }
            if let Some(pb) = &pb {
                pb.inc(1);
            }
        }
    }

    // --- Antigravity ---
    if process("antigravity") {
        if let Some(path) = &report.antigravity_history {
            if let Some(pb) = &pb {
                pb.set_message(format!("antigravity: {}", path.display()));
            }
            match parsers::antigravity::parse_file(path) {
                Ok(sessions) => {
                    let limited: Vec<_> = match opts.limit_per_tool {
                        Some(l) => sessions.into_iter().take(l).collect(),
                        None => sessions,
                    };
                    for s in limited {
                        import_one(&repo, s, &mut stats).await;
                    }
                }
                Err(e) => stats
                    .errors
                    .push(format!("antigravity {}: {}", path.display(), e)),
            }
            if let Some(pb) = &pb {
                pb.inc(1);
            }
        }
    }

    // --- Codex ---
    if process("codex") {
        if let Some(history) = &report.codex_history {
            if let Some(pb) = &pb {
                pb.set_message(format!("codex: {}", history.display()));
            }
            match parsers::codex::parse_history(history, report.codex_state.as_deref()) {
                Ok(sessions) => {
                    let limited: Vec<_> = match opts.limit_per_tool {
                        Some(l) => sessions.into_iter().take(l).collect(),
                        None => sessions,
                    };
                    for s in limited {
                        import_one(&repo, s, &mut stats).await;
                    }
                }
                Err(e) => stats.errors.push(format!("codex {}: {}", history.display(), e)),
            }
            if let Some(pb) = &pb {
                pb.inc(1);
            }
        }
    }

    // --- Hermes ---
    if process("hermes") {
        for (idx, path) in report.hermes_session_files.iter().enumerate() {
            if let Some(limit) = opts.limit_per_tool {
                if idx >= limit {
                    break;
                }
            }
            if let Some(pb) = &pb {
                pb.set_message(format!("hermes: {}", path.display()));
            }
            match parsers::hermes::parse_file(path) {
                Ok(Some(session)) => import_one(&repo, session, &mut stats).await,
                Ok(None) => {}
                Err(e) => stats
                    .errors
                    .push(format!("hermes {}: {}", path.display(), e)),
            }
            if let Some(pb) = &pb {
                pb.inc(1);
            }
        }
    }

    if let Some(pb) = pb {
        pb.finish_with_message("session import done");
    }

    // --- Obsidian vault scan ---
    for vault in &report.obsidian_vaults {
        match altevra_vault::scan_vault(vault) {
            Ok(files) => {
                stats.vault_docs_scanned += files.len() as u64;
                // Parse to validate (free — just checksum + frontmatter).
                // Full embedding is left to the brain's vault_indexer job which
                // already runs every 15min; we just surface the count here.
                for f in files.iter().take(500) {
                    if altevra_vault::parse_document(&f.path).is_ok() {
                        stats.vault_docs_parsed += 1;
                    } else {
                        stats.errors.push(format!(
                            "vault parse failed: {}",
                            f.path.display()
                        ));
                    }
                }
            }
            Err(e) => stats
                .errors
                .push(format!("vault {} scan: {}", vault.display(), e)),
        }
    }

    // --- LLM summary phase (default on, opt-out via --no-llm-summary) ---
    if !opts.no_llm_summary {
        match summarize_recent(&repo, &mut stats).await {
            Ok(()) => {}
            Err(e) => {
                tracing::warn!(error = %e, "LLM summarization phase failed; sessions remain unsummarized");
                stats.errors.push(format!("llm_summary: {e}"));
            }
        }
    }

    Ok((report, stats))
}

async fn import_one(repo: &SessionsRepository<'_>, sess: ImportedSession, stats: &mut ImportStats) {
    let id = Uuid::new_v4();
    let row = SessionRow {
        id,
        tool: sess.tool_id.clone(),
        project_id: None,
        project_name: sess.project_name.clone(),
        started_at: sess.started_at,
        ended_at: sess.ended_at,
        summary: None,
        tokens_in_total: 0,
        tokens_out_total: 0,
        cost_usd_estimate: 0.0,
        turn_count: 0,
        metadata: serde_json::json!({
            "imported_at": Utc::now().to_rfc3339(),
            "model_hint": sess.model,
        }),
        external_id: Some(sess.external_id.clone()),
        imported_from: Some(sess.imported_from.to_string_lossy().to_string()),
    };

    match repo.upsert_imported(&row).await {
        Ok(Some(actual_id)) => {
            *stats.by_tool.entry(sess.tool_id.clone()).or_insert(0) += 1;
            stats.sessions_imported += 1;
            for turn in &sess.turns {
                // Redact + auto-capture secrets before persisting.
                let store = altevra_secrets::SecretStore::new_keyring("altevra");
                let captures =
                    altevra_secrets::auto_capture(&turn.content, &store).unwrap_or_default();
                stats.secrets_captured += captures.len() as u64;
                let content = altevra_secrets::redact(&turn.content);

                let trow = TurnRow {
                    id: Uuid::new_v4(),
                    session_id: actual_id,
                    turn_idx: turn.turn_idx,
                    role: turn.role.clone(),
                    content,
                    tool_calls: turn.tool_calls.clone(),
                    tool_name: turn.tool_name.clone(),
                    model: turn.model.clone(),
                    tokens_in: turn.tokens_in,
                    tokens_out: turn.tokens_out,
                    latency_ms: turn.latency_ms,
                    file_changes: None,
                    redacted_count: captures.len() as i64,
                    created_at: turn.created_at,
                };
                if let Err(e) = repo.record_turn(&trow).await {
                    stats.errors.push(format!(
                        "record_turn {}/{}: {}",
                        sess.tool_id, sess.external_id, e
                    ));
                    return;
                }
                stats.turns_imported += 1;
            }
        }
        Ok(None) => {
            stats.sessions_skipped += 1;
        }
        Err(e) => stats.errors.push(format!(
            "upsert {}/{}: {}",
            sess.tool_id, sess.external_id, e
        )),
    }
}

/// Walk recently imported sessions without `summary` set, build a compact
/// prompt from their turns, call Gemini Flash, store result.
async fn summarize_recent(
    repo: &SessionsRepository<'_>,
    stats: &mut ImportStats,
) -> anyhow::Result<()> {
    let chat = match altevra_llm::GeminiFlashChat::from_secrets_or_env() {
        Ok(c) => c,
        Err(e) => {
            tracing::info!(error = %e, "no Gemini key — skipping LLM summary phase");
            return Ok(());
        }
    };

    // Pull last 200 imported sessions without a summary.
    let candidates = repo.list_sessions(None, None, 200).await?;
    let needing = candidates
        .into_iter()
        .filter(|s| s.summary.is_none() && s.external_id.is_some())
        .take(50); // cap LLM cost per analyze run

    // Modest pacing — Gemini Flash free tier is ~15 RPM. Use the rate limiter
    // from altevra-llm directly.
    let limiter = altevra_llm::RateLimiter::per_minute(15);

    for sess in needing {
        let turns = repo.list_turns(sess.id, 30).await?;
        if turns.is_empty() {
            continue;
        }
        let mut prompt = String::new();
        prompt.push_str("Session turns (truncated for summary):\n\n");
        for t in turns.iter().take(20) {
            let preview: String = t.content.chars().take(400).collect();
            prompt.push_str(&format!("[{}] {}\n", t.role, preview));
        }

        limiter.acquire().await;
        match chat.summarize(&prompt, 120).await {
            Ok(text) => {
                let clean = text.trim();
                if !clean.is_empty() {
                    repo.set_summary(sess.id, clean).await?;
                    stats.llm_summaries += 1;
                }
            }
            Err(e) => {
                stats
                    .errors
                    .push(format!("summarize {}: {}", sess.id, e));
            }
        }
    }
    Ok(())
}

pub fn print_report(report: &DiscoveryReport, stats: &ImportStats) {
    println!("\n=== Analyze Everything — Report ===");
    println!("  Discovery:");
    println!("    Claude Code JSONL files:   {}", report.claude_code_files.len());
    println!(
        "    Codex state.sqlite:        {}",
        if report.codex_state.is_some() { "found" } else { "—" }
    );
    println!(
        "    Codex history.jsonl:       {}",
        if report.codex_history.is_some() { "found" } else { "—" }
    );
    println!("    Cursor chatSessions:       {}", report.cursor_jsonl_files.len());
    println!(
        "    Antigravity history.jsonl: {}",
        if report.antigravity_history.is_some() { "found" } else { "—" }
    );
    println!("    Hermes session_*.json:     {}", report.hermes_session_files.len());
    println!("    Obsidian vaults:           {}", report.obsidian_vaults.len());
    println!("  Imported:");
    println!("    Sessions:        {}", stats.sessions_imported);
    println!("    Sessions skipped (duplicates): {}", stats.sessions_skipped);
    println!("    Turns:           {}", stats.turns_imported);
    println!("    Secrets captured: {}", stats.secrets_captured);
    println!("    LLM summaries:   {}", stats.llm_summaries);
    println!("    Vault docs scanned: {}", stats.vault_docs_scanned);
    println!("    Vault docs parsed:  {}", stats.vault_docs_parsed);
    if !stats.by_tool.is_empty() {
        println!("  By tool:");
        for (tool, n) in &stats.by_tool {
            println!("    {tool:14} {n}");
        }
    }
    if !stats.errors.is_empty() {
        println!("  Errors: {}", stats.errors.len());
        for e in stats.errors.iter().take(5) {
            println!("    ! {e}");
        }
        if stats.errors.len() > 5 {
            println!("    (... {} more)", stats.errors.len() - 5);
        }
    }
}

/// Convenience helper used by the CLI command.
pub fn open_pool(db: &PathBuf) -> sqlx::sqlite::SqliteConnectOptions {
    sqlx::sqlite::SqliteConnectOptions::new()
        .filename(db)
        .create_if_missing(true)
}

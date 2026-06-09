//! `altevra hook handle <event-name>` — parse a tool-emitted hook payload
//! from stdin and translate it into a `turn record` row.
//!
//! Used by Claude Code / Codex / Cursor / Antigravity hook configs that pipe
//! the tool's hook event JSON into Altevra. Schema per tool:
//!
//! * Claude Code `UserPromptSubmit`: `{"user_prompt": "..."}`
//! * Claude Code `PostToolUse`: `{"tool_name": "...", "tool_input": {...}, "tool_response": "..."}`
//! * Claude Code `SessionStart`: emits a new session id and writes the
//!   current-session pointer.
//! * Claude Code `Stop` / `SessionEnd`: closes the current session.
//!
//! Codex/Cursor schemas are similar enough — we read common fields (`prompt`,
//! `tool_name`, `command`, `content`) defensively.

use altevra_db::{
    create_pool, run_migrations, signal_for_session, EventsRepository, ImprovementSignalsRepository,
    SessionRow, SessionsRepository, TurnRow,
};
use altevra_secrets::{auto_capture, guard_text, SecretStore};
use chrono::Utc;
use clap::Args;
use std::io::Read;
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Args)]
pub struct HookHandleArgs {
    /// Event name (e.g. user_prompt_submit, post_tool_use, session_start, session_end).
    pub event: String,

    /// Tool emitting the event (defaults to claude-code).
    #[arg(long, default_value = "claude-code")]
    pub tool: String,

    /// Project name override.
    #[arg(long)]
    pub project: Option<String>,

    /// SQLite database path.
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,

    /// Skip stdin reading (for tests).
    #[arg(long)]
    pub no_stdin: bool,
}

pub async fn run(args: HookHandleArgs) -> anyhow::Result<()> {
    // Read stdin payload (tool hook system pipes JSON in).
    let payload = if args.no_stdin {
        serde_json::json!({})
    } else {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf).ok();
        serde_json::from_str::<serde_json::Value>(&buf)
            .unwrap_or(serde_json::Value::Object(Default::default()))
    };

    // Maintenance lock (db unify in progress): a hook must NEVER block the
    // host tool and must NOT write the database mid-merge — spool the event
    // (guard-redacted, one file per event) and exit 0. `altevra db
    // replay-spool` drains it after unify. Spool errors are non-fatal too.
    if altevra_core::maintenance::maintenance_locked_default() {
        if let Err(e) = spool_during_maintenance(&args, &payload) {
            eprintln!("[altevra] hook spool failed (non-fatal): {e}");
        }
        return Ok(());
    }

    // session_start is dispatched BEFORE the shared pool open: its entire DB
    // work (pool + migrations + insert + context assembly) runs under an
    // internal ≤1s deadline with a catch-all, so a locked/slow DB can never
    // stall or fail the hook (§P2.3). It always prints valid (possibly empty)
    // output and exits 0.
    if args.event == "session_start" {
        if let Some(out) = run_session_start(&args).await {
            println!("{out}");
        }
        return Ok(());
    }

    let pool = create_pool(&args.db.to_string_lossy()).await?;
    run_migrations(&pool).await?;
    let repo = SessionsRepository::new(&pool);

    match args.event.as_str() {
        "session_end" | "stop" => handle_session_end(&pool, &repo, &args, &payload).await?,
        "user_prompt_submit" => handle_user_prompt(&pool, &repo, &args, &payload).await?,
        "post_tool_use" => handle_post_tool_use(&pool, &repo, &args, &payload).await?,
        "pre_tool_use" => handle_pre_tool_use(&repo, &args, &payload).await?,
        other => {
            eprintln!("[altevra] unknown hook event: {other}");
        }
    }
    Ok(())
}

/// Spool path for hook events fired while `db unify` holds the maintenance
/// lock. Mirrors the live handlers' event mapping, but writes ONE guarded
/// JSON file per event (O_EXCL, 0600, $HOME-anchored) instead of touching
/// the database. Session pointer files are still maintained (they are plain
/// files, not DB rows) so turn events can resolve their session id, and the
/// per-tool stdout contract ([`session_start_stdout`], §P2.2) is preserved —
/// with the injected context degraded to empty while the lock is held.
fn spool_during_maintenance(
    args: &HookHandleArgs,
    payload: &serde_json::Value,
) -> anyhow::Result<()> {
    use crate::commands::db::{build_spool_turn, write_spool_entry, SpoolEntry};
    use altevra_core::security::Sensitivity;

    let dir = altevra_core::maintenance::spool_dir();
    match args.event.as_str() {
        "session_start" => {
            let id = Uuid::new_v4();
            write_spool_entry(
                &dir,
                &args.tool,
                &SpoolEntry::SessionStart {
                    tool: args.tool.clone(),
                    session_id: id,
                    project_name: args.project.clone(),
                    started_at: Utc::now(),
                    working_dir: resolve_working_dir(),
                },
            )?;
            write_current_session(&id, &args.tool)?;
            // Same §P2.2 stdout contract as the live path; the injected
            // context degrades to EMPTY while the DB is maintenance-locked.
            if let Some(out) = session_start_stdout(&args.tool, &id, "") {
                println!("{out}");
            }
        }
        "session_end" | "stop" => {
            if let Some(id) = read_current_session(&args.tool)? {
                // Guard the summary BEFORE it reaches disk.
                let summary = payload
                    .get("summary")
                    .and_then(serde_json::Value::as_str)
                    .map(|s| guard_text(s, Sensitivity::Internal).value);
                write_spool_entry(
                    &dir,
                    &args.tool,
                    &SpoolEntry::SessionEnd {
                        tool: args.tool.clone(),
                        session_id: id,
                        summary,
                        ended_at: Utc::now(),
                    },
                )?;
                clear_current_session(&args.tool)?;
                println!("{{\"closed_session\":\"{id}\"}}");
            }
        }
        ev @ ("user_prompt_submit" | "post_tool_use" | "pre_tool_use") => {
            // No active session → same silent skip as the live path.
            if let Some(session_id) = read_current_session(&args.tool)? {
                if let Some(entry) =
                    build_spool_turn(&args.tool, session_id, ev, payload, resolve_working_dir())
                {
                    write_spool_entry(&dir, &args.tool, &entry)?;
                }
            }
        }
        other => {
            eprintln!("[altevra] unknown hook event (not spooled): {other}");
        }
    }
    Ok(())
}

/// Resolve the working directory for the current hook invocation.
///
/// Priority:
///   1. `$CLAUDE_PROJECT_DIR` env var (set by Claude Code ≥1.x for the project root).
///   2. `std::env::current_dir()` — the directory the hook was invoked from.
///   3. `None` if neither is available (sandboxed / unavailable env).
///
/// The result is always an absolute path string, or None.
fn resolve_working_dir() -> Option<String> {
    // Prefer the project-level dir set by the host tool.
    if let Ok(p) = std::env::var("CLAUDE_PROJECT_DIR") {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    // Fall back to the process cwd at hook-fire time.
    std::env::current_dir()
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

/// Hard deadline for the ENTIRE session_start DB path (pool + migrations +
/// insert + context assembly). Below busy_timeout (5s) by design — a
/// write-locked DB degrades to empty context instead of stalling the host
/// tool (§P2.3).
const SESSION_START_DEADLINE_MS: u64 = 900;

/// session_start, fail-open end to end (§P2.2/§P2.3). Returns the EXACT
/// stdout document (or `None` for codex — its stdout is user-visible and
/// clobbers the TUI). Never errors: any DB failure or deadline overrun
/// degrades the injected context to the empty string, the pointer file is
/// still written, and the hook exits 0.
async fn run_session_start(args: &HookHandleArgs) -> Option<String> {
    use altevra_core::session_context::{session_start_transport, SessionStartTransport};

    let id = Uuid::new_v4();
    let working_dir = resolve_working_dir();
    // Only the hook-additionalContext transport (claude-code) assembles a
    // block — the channel decision lives in ONE place (§P2.1).
    let wants_block = matches!(
        session_start_transport(&args.tool),
        SessionStartTransport::HookAdditionalContext
    );

    let deadline = std::time::Duration::from_millis(SESSION_START_DEADLINE_MS);
    let assembled = tokio::time::timeout(
        deadline,
        session_start_db_work(args, id, working_dir, wants_block),
    )
    .await;
    let block = match assembled {
        Ok(Ok(block)) => block,
        Ok(Err(e)) => {
            eprintln!("[altevra] session_start db work failed (non-fatal): {e}");
            String::new()
        }
        Err(_) => {
            eprintln!(
                "[altevra] session_start exceeded {SESSION_START_DEADLINE_MS}ms — \
                 context degraded to empty (non-fatal)"
            );
            String::new()
        }
    };

    // The pointer file is plain-filesystem — written even when the DB row
    // didn't land (record_turn already downgrades the resulting FK miss).
    if let Err(e) = write_current_session(&id, &args.tool) {
        eprintln!("[altevra] session pointer write failed (non-fatal): {e}");
    }
    session_start_stdout(&args.tool, &id, &block)
}

/// Everything session_start does against the DB, so the caller can put ONE
/// deadline around it. Returns the rendered context block ("" when the tool's
/// transport doesn't want one).
async fn session_start_db_work(
    args: &HookHandleArgs,
    id: Uuid,
    working_dir: Option<String>,
    wants_block: bool,
) -> anyhow::Result<String> {
    use altevra_core::events::{ActorType, Event, EventType};

    let pool = create_pool(&args.db.to_string_lossy()).await?;
    run_migrations(&pool).await?;
    let repo = SessionsRepository::new(&pool);
    let row = SessionRow {
        id,
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
        metadata: serde_json::json!({"started_via": "hook"}),
        external_id: None,
        imported_from: None,
        working_dir,
    };
    repo.start_session(&row).await?;

    // Populate the events table so the observer pipeline has data to detect patterns.
    // Best-effort: a failure here must NOT block the session start.
    let ev = Event::new(
        EventType::SessionStarted,
        &format!("{} session started", args.tool),
        "hook_handle",
        ActorType::System,
    );
    if let Err(e) = EventsRepository::new(&pool).insert(&ev).await {
        eprintln!("[altevra] events insert failed (non-fatal): {e}");
    }

    if !wants_block {
        return Ok(String::new());
    }
    // Gated + audited assembly (§P2.4); an assembly error degrades to empty
    // (fail-open for availability) while per-item filtering stays fail-closed
    // inside the gather layer.
    let data = altevra_bootstrap::session_context::gather_session_context(
        &pool,
        &format!("session_start:{id}"),
        None,
    )
    .await;
    Ok(altevra_core::session_context::render_session_context_block(&data))
}

/// The §P2.2 stdout contract, decided in ONE place keyed by `--tool`:
///  * claude-code → EXACTLY ONE JSON document shaped per the Claude Code
///    hooks spec (`hookSpecificOutput.additionalContext`); the session id
///    moves to stderr (never a second JSON object on stdout).
///  * codex → `None` (NOTHING on stdout — user-visible, clobbers the TUI).
///  * everything else (hermes/cursor/unknown) → the legacy `{"session_id"}`
///    document (hermes pulls context via the MCP bootstrap packet, cursor via
///    `altevra context --session-block`).
pub(crate) fn session_start_stdout(tool: &str, id: &Uuid, block: &str) -> Option<String> {
    use altevra_core::session_context::{session_start_transport, SessionStartTransport};
    match session_start_transport(tool) {
        SessionStartTransport::HookAdditionalContext => {
            eprintln!("[altevra] session_id={id}");
            Some(
                serde_json::json!({
                    "hookSpecificOutput": {
                        "hookEventName": "SessionStart",
                        "additionalContext": block,
                    }
                })
                .to_string(),
            )
        }
        SessionStartTransport::Nothing => {
            eprintln!("[altevra] session_id={id}");
            None
        }
        SessionStartTransport::BootstrapPacket | SessionStartTransport::PullCli => {
            Some(format!("{{\"session_id\":\"{id}\"}}"))
        }
    }
}

async fn handle_session_end(
    pool: &sqlx::SqlitePool,
    repo: &SessionsRepository<'_>,
    args: &HookHandleArgs,
    payload: &serde_json::Value,
) -> anyhow::Result<()> {
    use altevra_core::events::{ActorType, Event, EventType};

    let summary = payload
        .get("summary")
        .and_then(serde_json::Value::as_str)
        .map(String::from);
    if let Some(id) = read_current_session(&args.tool)? {
        repo.end_session(id, summary.as_deref()).await?;

        // Populate events table with SessionEnded (best-effort, non-fatal).
        let ev = Event::new(
            EventType::SessionEnded,
            &format!("{} session ended", args.tool),
            "hook_handle",
            ActorType::System,
        );
        if let Err(e) = EventsRepository::new(pool).insert(&ev).await {
            eprintln!("[altevra] events insert failed (non-fatal): {e}");
        }

        // Real-time self-improve producer (C1): one cheap improvement_signal per
        // session ingest — the orchestrator (a later seam) clusters open signals
        // into proposals. SI-6 self-write exclusion: a resident-mode-authored
        // session enqueues NOTHING (signal_for_session returns None), so
        // Altevra's own output never feeds the loop back into itself. Best-effort:
        // a signal-enqueue failure must NOT block closing the session.
        enqueue_session_signal(pool, repo, id).await;
        clear_current_session(&args.tool)?;
        println!("{{\"closed_session\":\"{id}\"}}");
    }
    Ok(())
}

/// Enqueue the per-session improvement signal (C1 producer). Reads the closed
/// session's provenance (`tool`/`project`/`turn_count`) and asks the pure
/// [`signal_for_session`] producer for a signal — which is `None` when SI-6
/// excludes a resident-authored session. Best-effort: any error is logged to
/// stderr and swallowed so the hook never fails the agent's session close.
async fn enqueue_session_signal(pool: &sqlx::SqlitePool, repo: &SessionsRepository<'_>, id: Uuid) {
    let session = match repo.get_session(id).await {
        Ok(Some(s)) => s,
        Ok(None) => return,
        Err(e) => {
            eprintln!("[altevra] signal enqueue skipped (session lookup failed): {e}");
            return;
        }
    };
    // SI-6 is enforced inside signal_for_session: resident-authored → None.
    let Some(new_signal) =
        signal_for_session(&id.to_string(), &session.tool, session.project_name.as_deref(), session.turn_count)
    else {
        return;
    };
    let signals = ImprovementSignalsRepository::new(pool);
    if let Err(e) = signals.insert(&new_signal).await {
        eprintln!("[altevra] improvement_signal enqueue failed (non-fatal): {e}");
    }
}

async fn handle_user_prompt(
    pool: &sqlx::SqlitePool,
    repo: &SessionsRepository<'_>,
    args: &HookHandleArgs,
    payload: &serde_json::Value,
) -> anyhow::Result<()> {
    let content =
        first_field(payload, &["user_prompt", "prompt", "content", "message"]).unwrap_or_default();
    record_turn(repo, "user", &content, None, &args.tool, payload).await?;
    // P3c: a user prompt inside an open skill-invocation judgment window is a
    // "reaction" — emit skill_reaction (best-effort, never blocks the hook).
    if let Ok(Some(session_id)) = read_current_session(&args.tool) {
        maybe_emit_skill_reaction(pool, session_id).await;
    }
    Ok(())
}

/// Emit a `skill_reaction` event if the session has a PENDING `skill_invocation`
/// whose K-message window isn't spent yet (K = [`SKILL_REACTION_WINDOW_K`]).
/// Content-free: the event carries only the invocation event id + skill slug —
/// the judge reads the actual turns from the turns table. Best-effort: any DB
/// error is swallowed (a reaction must never fail the host tool's hook).
async fn maybe_emit_skill_reaction(pool: &sqlx::SqlitePool, session_id: Uuid) {
    use altevra_core::events::{ActorType, Event, EventType};

    // Latest pending invocation for THIS session.
    let inv: Option<(String, String)> = sqlx::query_as(
        "SELECT id, COALESCE(json_extract(payload, '$.skill'), '') FROM events \
         WHERE event_type = 'skill_invocation' AND status = 'pending' \
           AND entity_type = 'session' AND entity_id = ? \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(session_id.to_string())
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    let Some((inv_id, skill)) = inv else { return };

    // Window budget: at most K reactions per invocation.
    let reactions: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE event_type = 'skill_reaction' \
         AND json_extract(payload, '$.invocation_event_id') = ?",
    )
    .bind(&inv_id)
    .fetch_one(pool)
    .await
    .unwrap_or(i64::MAX);
    if reactions >= SKILL_REACTION_WINDOW_K {
        return;
    }

    let ev = Event::new(
        EventType::SkillReaction,
        &format!("reaction to skill '{skill}'"),
        "hook_handle",
        ActorType::User,
    )
    .with_entity("session", session_id.to_string())
    .with_payload(serde_json::json!({
        "invocation_event_id": inv_id,
        "skill": skill,
    }));
    if let Err(e) = EventsRepository::new(pool).insert(&ev).await {
        eprintln!("[altevra] skill_reaction event insert failed (non-fatal): {e}");
    }
}

/// K-message judgment window (Hivemind `DEFAULT_JUDGE_WINDOW`, PLAN-ALIVE §P3c).
pub(crate) const SKILL_REACTION_WINDOW_K: i64 = 3;

async fn handle_pre_tool_use(
    repo: &SessionsRepository<'_>,
    args: &HookHandleArgs,
    payload: &serde_json::Value,
) -> anyhow::Result<()> {
    let tool_name = payload
        .get("tool_name")
        .and_then(serde_json::Value::as_str)
        .map(String::from);
    let content = payload
        .get("tool_input")
        .map(|v| v.to_string())
        .unwrap_or_default();
    record_turn(repo, "tool_call", &content, tool_name, &args.tool, payload).await
}

async fn handle_post_tool_use(
    pool: &sqlx::SqlitePool,
    repo: &SessionsRepository<'_>,
    args: &HookHandleArgs,
    payload: &serde_json::Value,
) -> anyhow::Result<()> {
    let tool_name = payload
        .get("tool_name")
        .and_then(serde_json::Value::as_str)
        .map(String::from);
    let response = first_field(payload, &["tool_response", "result", "output"]).unwrap_or_default();
    record_turn(
        repo,
        "tool_result",
        &response,
        tool_name.clone(),
        &args.tool,
        payload,
    )
    .await?;
    // P3c: a Skill tool call opens a K-message judgment window — emit
    // skill_invocation so the backward pass (skill_reaction_judge) can later
    // judge whether the skill's guidance actually worked. Best-effort.
    if tool_name.as_deref() == Some("Skill") {
        if let Ok(Some(session_id)) = read_current_session(&args.tool) {
            emit_skill_invocation(pool, session_id, payload).await;
        }
    }
    Ok(())
}

/// Emit a `skill_invocation` event for a Skill tool call. Payload is
/// content-free metadata only: the skill slug + the session's current turn
/// index (the judge pins the reaction window to this index, Hivemind
/// `toolUseId` semantics). Best-effort, never fails the hook.
async fn emit_skill_invocation(
    pool: &sqlx::SqlitePool,
    session_id: Uuid,
    payload: &serde_json::Value,
) {
    use altevra_core::events::{ActorType, Event, EventType};

    let skill = payload
        .get("tool_input")
        .and_then(|ti| {
            ti.get("skill")
                .or_else(|| ti.get("skill_name"))
                .or_else(|| ti.get("name"))
        })
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown")
        .to_string();

    // Pin the invocation to the just-recorded tool_result turn's index so a
    // quick re-invocation of the same skill can't shift the judged window.
    let turn_idx: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(turn_idx), 0) FROM turns WHERE session_id = ?",
    )
    .bind(session_id.to_string())
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    let ev = Event::new(
        EventType::SkillInvocation,
        &format!("skill '{skill}' invoked"),
        "hook_handle",
        ActorType::Agent,
    )
    .with_entity("session", session_id.to_string())
    .with_payload(serde_json::json!({
        "skill": skill,
        "session_id": session_id.to_string(),
        "invocation_turn_idx": turn_idx,
    }));
    if let Err(e) = EventsRepository::new(pool).insert(&ev).await {
        eprintln!("[altevra] skill_invocation event insert failed (non-fatal): {e}");
    }
}

/// Recursively scrub every string leaf of a JSON value through `guard_text`.
/// Tool inputs and file diffs are the richest source of raw secrets/PII (Edit
/// payloads, `export OPENAI_API_KEY=...`, customer emails in file contents), so
/// they must be scrubbed before persistence (R11 #2 — content-only redaction
/// left these sibling columns raw). Returns the scrubbed value, the count of
/// redacted leaves, and the worst-case sensitivity across all leaves.
pub(crate) fn guard_json(
    v: &serde_json::Value,
) -> (serde_json::Value, i64, altevra_core::security::Sensitivity) {
    use altevra_core::security::Sensitivity;
    use altevra_core::status::RedactionStatus;
    match v {
        serde_json::Value::String(s) => {
            let g = guard_text(s, Sensitivity::Internal);
            let n = i64::from(matches!(g.redaction_status, RedactionStatus::Redacted));
            (serde_json::Value::String(g.value), n, g.sensitivity)
        }
        serde_json::Value::Array(arr) => {
            let mut out = Vec::with_capacity(arr.len());
            let mut count = 0i64;
            let mut sens = Sensitivity::Internal;
            for item in arr {
                let (vv, c, sn) = guard_json(item);
                out.push(vv);
                count += c;
                sens = sens.combine(&sn);
            }
            (serde_json::Value::Array(out), count, sens)
        }
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            let mut count = 0i64;
            let mut sens = Sensitivity::Internal;
            for (k, vv) in map {
                let (rv, c, sn) = guard_json(vv);
                out.insert(k.clone(), rv);
                count += c;
                sens = sens.combine(&sn);
            }
            (serde_json::Value::Object(out), count, sens)
        }
        other => (other.clone(), 0, Sensitivity::Internal),
    }
}

async fn record_turn(
    repo: &SessionsRepository<'_>,
    role: &str,
    raw_content: &str,
    tool_name: Option<String>,
    source_tool: &str,
    payload: &serde_json::Value,
) -> anyhow::Result<()> {
    // Key session lookup by tool so concurrent Claude/Codex sessions from
    // different projects never share the same pointer file.
    let session_id = match read_current_session(source_tool)? {
        Some(id) => id,
        None => {
            // No active session — silently skip so the agent isn't blocked
            // when running outside the recorded lifecycle.
            return Ok(());
        }
    };

    // Auto-capture: any detected secret is persisted into the secret store
    // under a stable key (fingerprint-based) BEFORE the content gets redacted
    // out of chat history. This is Pavle's "don't make me re-send keys"
    // feature — Altevra grabs them once, stores forever, then redacts.
    let store = resolve_capture_store();
    let captures = auto_capture(raw_content, &store).unwrap_or_default();

    // PreWriteSafetyGate text path (T1.13): scrub BOTH secrets AND PII (emails)
    // and classify sensitivity before the turn is persisted. Previously only
    // secrets were redacted; PII leaked into stored content. guard_text raises
    // sensitivity (default-up) when credential/PII risk is present.
    let guarded = guard_text(raw_content, altevra_core::Sensitivity::Internal);
    let final_content = guarded.value;
    // redacted_count reflects everything scrubbed: captured secrets, plus a PII
    // bump when emails were redacted.
    let mut redacted_count = captures.len().max(guarded.sightings.len()) as i64;
    if guarded
        .risk_tags
        .contains(&altevra_core::RiskTag::ThirdPartyPii)
    {
        redacted_count += 1;
    }
    let mut turn_sensitivity = guarded.sensitivity.clone();
    let mut turn_redaction = guarded.redaction_status.clone();

    // R11 #2: scrub the side-channel columns (tool_input + file_changes) too —
    // never persist them raw. Worst-case sensitivity is folded into the turn.
    let guarded_tool_input = payload.get("tool_input").map(guard_json);
    let guarded_file_changes = payload.get("file_changes").map(guard_json);
    let mut side_channel_redacted = false;
    for g in [&guarded_tool_input, &guarded_file_changes]
        .into_iter()
        .flatten()
    {
        redacted_count += g.1;
        turn_sensitivity = turn_sensitivity.combine(&g.2);
        side_channel_redacted |= g.1 > 0;
    }
    if side_channel_redacted {
        turn_redaction = altevra_core::status::RedactionStatus::Redacted;
    }
    let scrubbed_tool_input = guarded_tool_input.map(|(v, _, _)| v);
    let scrubbed_file_changes = guarded_file_changes.map(|(v, _, _)| v);

    let capture_meta: Vec<serde_json::Value> = captures
        .iter()
        .map(|c| {
            serde_json::json!({
                "kind": format!("{:?}", c.kind).to_lowercase(),
                "key": c.key,
                "fingerprint": c.fingerprint,
                "was_new": c.was_new,
            })
        })
        .collect();

    // Capture the turn's own working_dir. If it differs from the session's
    // (Pavle's "run from ~, project elsewhere" case), record it explicitly so
    // the turn reflects where the hook actually fired.
    let turn_working_dir = resolve_working_dir();

    let turn_idx = repo.next_turn_idx(session_id).await?;
    let turn = TurnRow {
        id: Uuid::new_v4(),
        session_id,
        turn_idx,
        role: role.to_string(),
        content: final_content,
        tool_calls: if capture_meta.is_empty() {
            scrubbed_tool_input.clone()
        } else {
            Some(serde_json::json!({
                "tool_input": scrubbed_tool_input.clone(),
                "captured_secrets": capture_meta,
            }))
        },
        tool_name,
        model: payload
            .get("model")
            .and_then(serde_json::Value::as_str)
            .map(String::from),
        tokens_in: payload.get("tokens_in").and_then(serde_json::Value::as_i64),
        tokens_out: payload
            .get("tokens_out")
            .and_then(serde_json::Value::as_i64),
        latency_ms: payload
            .get("latency_ms")
            .and_then(serde_json::Value::as_i64),
        file_changes: scrubbed_file_changes,
        redacted_count,
        source_tool: Some(source_tool.to_string()),
        sensitivity: turn_sensitivity.to_string(),
        redaction_status: turn_redaction.to_string(),
        created_at: Utc::now(),
        working_dir: turn_working_dir,
    };
    // FK robustness: a FOREIGN KEY constraint error means the session_id in
    // the pointer file doesn't exist in *this* DB (path mismatch or leftover
    // stale pointer). Treat as a warning — the hook must NEVER exit non-zero
    // and block the host tool. Any other DB error is also downgraded.
    if let Err(e) = repo.record_turn(&turn).await {
        let msg = e.to_string();
        if msg.contains("FOREIGN KEY") || msg.contains("foreign key") {
            eprintln!("[altevra] turn not recorded (FK mismatch — stale session pointer?): {msg}");
        } else {
            eprintln!("[altevra] turn record failed (non-fatal): {msg}");
        }
    }
    Ok(())
}

/// Resolve which SecretStore backend auto-capture should use.
///
/// Priority:
///   1. `ALTEVRA_SECRETS_FILE` env var → encrypted file at that path.
///   2. `~/.altevra/secrets.enc` if `ALTEVRA_SECRETS_KEY` env is set.
///   3. OS keyring (default).
fn resolve_capture_store() -> SecretStore {
    if let Ok(path) = std::env::var("ALTEVRA_SECRETS_FILE") {
        let key_env = std::env::var("ALTEVRA_SECRETS_KEY_ENV")
            .unwrap_or_else(|_| "ALTEVRA_SECRETS_KEY".into());
        return SecretStore::new_encrypted_file("altevra", path.into(), &key_env);
    }
    if std::env::var("ALTEVRA_SECRETS_KEY").is_ok() {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        let path = std::path::PathBuf::from(home).join(".altevra/secrets.enc");
        return SecretStore::new_encrypted_file("altevra", path, "ALTEVRA_SECRETS_KEY");
    }
    SecretStore::new_keyring("altevra")
}

fn first_field(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Some(s) = value.get(*k).and_then(serde_json::Value::as_str) {
            return Some(s.to_string());
        }
    }
    None
}

fn write_current_session(id: &Uuid, tool: &str) -> anyhow::Result<()> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let path = altevra_core::current_session_path(tool, &cwd);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, id.to_string())?;
    Ok(())
}

fn read_current_session(tool: &str) -> anyhow::Result<Option<Uuid>> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let path = altevra_core::current_session_path(tool, &cwd);
    if !path.exists() {
        return Ok(None);
    }
    let s = std::fs::read_to_string(&path)?;
    Ok(Uuid::parse_str(s.trim()).ok())
}

fn clear_current_session(tool: &str) -> anyhow::Result<()> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let path = altevra_core::current_session_path(tool, &cwd);
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Seed a closed session row with a given tool + turn count, returning its id.
    async fn seed_session(
        pool: &sqlx::SqlitePool,
        tool: &str,
        project: Option<&str>,
        turns: i64,
    ) -> Uuid {
        let repo = SessionsRepository::new(pool);
        let id = Uuid::new_v4();
        repo.start_session(&SessionRow {
            id,
            tool: tool.to_string(),
            project_id: None,
            project_name: project.map(String::from),
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
            working_dir: None,
        })
        .await
        .unwrap();
        for i in 0..turns {
            repo.record_turn(&TurnRow {
                id: Uuid::new_v4(),
                session_id: id,
                turn_idx: i,
                role: "user".into(),
                content: format!("turn {i}"),
                tool_calls: None,
                tool_name: None,
                model: None,
                tokens_in: None,
                tokens_out: None,
                latency_ms: None,
                file_changes: None,
                redacted_count: 0,
                source_tool: Some(tool.to_string()),
                sensitivity: altevra_core::Sensitivity::Internal.to_string(),
                redaction_status: altevra_core::status::RedactionStatus::Clean.to_string(),
                created_at: Utc::now(),
                working_dir: None,
            })
            .await
            .unwrap();
        }
        repo.end_session(id, None).await.unwrap();
        id
    }

    #[tokio::test]
    async fn session_ingest_enqueues_exactly_one_signal() {
        // C1 producer: a real external-tool session ingest enqueues exactly ONE
        // improvement_signal; running the producer again is idempotent (no 2nd row).
        let tmp = TempDir::new().unwrap();
        let pool = create_pool(&tmp.path().join("a.db").to_string_lossy())
            .await
            .unwrap();
        run_migrations(&pool).await.unwrap();
        let repo = SessionsRepository::new(&pool);
        let id = seed_session(&pool, "claude-code", Some("altevra"), 3).await;

        enqueue_session_signal(&pool, &repo, id).await;
        let signals = ImprovementSignalsRepository::new(&pool);
        let open = signals.list_open().await.unwrap();
        assert_eq!(open.len(), 1, "exactly one signal per session ingest");
        assert_eq!(open[0].kind, "session_ingest");
        assert_eq!(open[0].source_ref, format!("session:{id}"));
        assert_eq!(
            open[0].cluster_key.as_deref(),
            Some("session:claude-code:altevra")
        );

        // Idempotent: re-running the producer for the same session does NOT add a
        // second row (stable dedup id).
        enqueue_session_signal(&pool, &repo, id).await;
        assert_eq!(
            signals.list_open().await.unwrap().len(),
            1,
            "producer re-run is idempotent"
        );
    }

    #[tokio::test]
    async fn resident_authored_session_enqueues_no_signal_si6() {
        // SI-6: a session authored by a resident mode must NEVER become a signal
        // (no self-feedback loop). The producer enqueues ZERO rows for it.
        let tmp = TempDir::new().unwrap();
        let pool = create_pool(&tmp.path().join("b.db").to_string_lossy())
            .await
            .unwrap();
        run_migrations(&pool).await.unwrap();
        let repo = SessionsRepository::new(&pool);
        let id = seed_session(&pool, "resident:observer", None, 2).await;

        enqueue_session_signal(&pool, &repo, id).await;
        let signals = ImprovementSignalsRepository::new(&pool);
        assert_eq!(
            signals.list_open().await.unwrap().len(),
            0,
            "SI-6: resident-authored session enqueues nothing"
        );
    }

    /// Serializes every HOME-mutating test in this binary — $HOME is process
    /// global, so two parallel HomeGuards would corrupt each other's paths.
    static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Panic-safe HOME override: restores the previous HOME on drop so a
    /// failing assertion can't leak a TempDir HOME into sibling tests.
    struct HomeGuard {
        prev: Option<std::ffi::OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl HomeGuard {
        fn set(home: &std::path::Path) -> Self {
            // A panicked sibling poisons the lock; the env value is restored
            // by its Drop, so the poison itself is harmless — clear it.
            let lock = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let prev = std::env::var_os("HOME");
            std::env::set_var("HOME", home);
            Self { prev, _lock: lock }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(h) => std::env::set_var("HOME", h),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    #[tokio::test]
    async fn session_start_writes_pointer_file() {
        let tmp = TempDir::new().unwrap();
        // The session pointer anchors at $HOME (`current_session_path`), so
        // override HOME to the TempDir — this test must NEVER write/remove
        // files under the real ~/.altevra/state/.
        let _home = HomeGuard::set(tmp.path());
        let cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let tool = "claude-code";
        let args = HookHandleArgs {
            event: "session_start".into(),
            tool: tool.to_string(),
            project: Some("altevra".into()),
            db: tmp.path().join("altevra.db"),
            no_stdin: true,
        };
        run(args).await.unwrap();
        // The session pointer is now $HOME/.altevra/state/session-<tool>-<cwd_hash>.txt
        let ptr = altevra_core::current_session_path(tool, tmp.path());
        assert!(ptr.exists(), "session pointer file must exist after session_start");

        // session_end clears pointer
        let args = HookHandleArgs {
            event: "session_end".into(),
            tool: tool.to_string(),
            project: None,
            db: tmp.path().join("altevra.db"),
            no_stdin: true,
        };
        run(args).await.unwrap();
        assert!(!ptr.exists(), "session pointer file must be removed after session_end");

        // Prove isolation: everything the run produced lives under the TempDir.
        assert!(
            ptr.starts_with(tmp.path()),
            "session pointer must be anchored under the overridden HOME, got {}",
            ptr.display()
        );

        std::env::set_current_dir(cwd).unwrap();
    }

    // -----------------------------------------------------------------------
    // P2 hermetic gate — SessionStart context injection
    // -----------------------------------------------------------------------

    fn hook_args(tool: &str, db: std::path::PathBuf) -> HookHandleArgs {
        HookHandleArgs {
            event: "session_start".into(),
            tool: tool.into(),
            project: Some("altevra".into()),
            db,
            no_stdin: true,
        }
    }

    /// Seed a decision row in the object-envelope store P2 queries.
    async fn seed_decision(
        pool: &sqlx::SqlitePool,
        id: &str,
        title: &str,
        domain: &str,
        sens: &str,
        red: &str,
    ) {
        altevra_db::ObjectIndexRepository::new(pool)
            .upsert(&altevra_db::ObjectIndexRow {
                object_type: "decision".into(),
                id: id.into(),
                status: "active".into(),
                sensitivity: sens.into(),
                domain: domain.into(),
                scope: None,
                title: Some(title.into()),
                categories: "[\"business\"]".into(),
                tags: "[]".into(),
                redaction_status: red.into(),
                updated_at: Utc::now(),
            })
            .await
            .unwrap();
    }

    /// P2 gate: for claude-code the hook emits EXACTLY ONE JSON document,
    /// shaped per the Claude Code hooks spec, whose `additionalContext`
    /// carries goals + decisions + the tool register — and a Restricted/
    /// high-water decision is NOT in it. Every injected item has an
    /// exposure_decisions audit row; the block fits the 2K budget.
    #[tokio::test]
    async fn p2_claude_code_block_gated_audited_single_json_doc() {
        let tmp = TempDir::new().unwrap();
        let _home = HomeGuard::set(tmp.path());
        let db = tmp.path().join("p2.db");
        let pool = create_pool(&db.to_string_lossy()).await.unwrap();
        run_migrations(&pool).await.unwrap();

        // goals.json at the $HOME-anchored default path (HomeGuard isolates).
        let goals = tmp.path().join(".altevra/state/goals.json");
        std::fs::create_dir_all(goals.parent().unwrap()).unwrap();
        std::fs::write(
            &goals,
            serde_json::json!([{"title": "2 paying Simple Surplus clients"}]).to_string(),
        )
        .unwrap();
        // an injectable business decision + a Restricted high-water one.
        seed_decision(&pool, "d1", "ONE canonical DB", "business", "internal", "clean").await;
        seed_decision(
            &pool,
            "d_secret",
            "Private health decision",
            "health",
            "restricted",
            "clean",
        )
        .await;
        // a curated tool for the register section.
        let mut t = altevra_db::ToolRecordRow::new("imperium-crawl", "cli");
        t.invocation = serde_json::json!({"canonical": "imperium-crawl <cmd>"});
        t.source = "manual".into();
        altevra_db::ToolRecordsRepository::new(&pool)
            .upsert(&t)
            .await
            .unwrap();

        let out = run_session_start(&hook_args("claude-code", db))
            .await
            .expect("claude-code emits a stdout document");

        // EXACTLY ONE valid JSON document (a 2nd object would fail the parse).
        let doc: serde_json::Value =
            serde_json::from_str(&out).expect("stdout must be one valid JSON document");
        assert_eq!(doc["hookSpecificOutput"]["hookEventName"], "SessionStart");
        let block = doc["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .expect("additionalContext present");
        // session_id moved OFF stdout (§P2.2) — never a second JSON object.
        assert!(!out.contains("\"session_id\""));

        // content: goals + decisions + tool register.
        assert!(block.contains("2 paying Simple Surplus clients"), "goal missing: {block}");
        assert!(block.contains("ONE canonical DB"), "decision missing: {block}");
        assert!(block.contains("=== ALTEVRA TOOL REGISTER ==="), "register missing");
        assert!(block.contains("imperium-crawl (cli): imperium-crawl <cmd>"));
        // THE leak assertion: the Restricted/high-water decision is NOT injected.
        assert!(
            !block.contains("Private health decision"),
            "Restricted decision leaked into session context"
        );
        // §P2.5: budget pinned ≤ 2K tokens.
        assert!(
            altevra_core::session_context::estimate_tokens(block)
                <= altevra_core::session_context::SESSION_BLOCK_TOKEN_BUDGET
        );

        // §P2.4: every evaluated item wrote an exposure_decisions audit row
        // (1 goal + 2 decisions [1 included + 1 excluded] + 1 tool = 4).
        let audit_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM exposure_decisions WHERE packet_id LIKE 'session_start:%'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(audit_count, 4, "one audit row per evaluated item");
    }

    /// P2 gate: codex gets NOTHING on stdout (user-visible — clobbers the TUI),
    /// while non-injection tools keep the legacy `{"session_id"}` document.
    #[tokio::test]
    async fn p2_codex_emits_nothing_others_keep_legacy_stdout() {
        let tmp = TempDir::new().unwrap();
        let _home = HomeGuard::set(tmp.path());

        let out = run_session_start(&hook_args("codex", tmp.path().join("codex.db"))).await;
        assert!(out.is_none(), "codex must print NOTHING on stdout");
        // the session pointer still exists so turn capture keeps working.
        let cwd = std::env::current_dir().unwrap();
        assert!(altevra_core::current_session_path("codex", &cwd).exists());

        // cursor (pull transport) keeps the legacy single-line session_id doc.
        let out = run_session_start(&hook_args("cursor", tmp.path().join("cursor.db")))
            .await
            .expect("cursor keeps legacy stdout");
        let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(doc["session_id"].is_string());
        assert!(doc.get("hookSpecificOutput").is_none());
    }

    /// P2 gate: a write-locked DB must neither stall nor fail the hook — the
    /// deadline (≤1s) degrades the context to EMPTY, output stays a single
    /// valid JSON document, and the handler returns Ok (exit 0).
    #[tokio::test(flavor = "multi_thread")]
    async fn p2_locked_db_degrades_to_empty_within_deadline() {
        let tmp = TempDir::new().unwrap();
        let _home = HomeGuard::set(tmp.path());
        let db = tmp.path().join("locked.db");
        // pre-migrate so the lock contends on the WRITE path, not migration.
        let pool = create_pool(&db.to_string_lossy()).await.unwrap();
        run_migrations(&pool).await.unwrap();
        pool.close().await;

        // hold an exclusive write lock for the duration of the hook call.
        let lock = rusqlite::Connection::open(&db).unwrap();
        lock.execute_batch("BEGIN IMMEDIATE;").unwrap();

        let started = std::time::Instant::now();
        let out = run_session_start(&hook_args("claude-code", db.clone())).await;
        let elapsed = started.elapsed();
        drop(lock);

        // fail-open: still exactly one valid JSON doc, context EMPTY.
        let out = out.expect("locked DB still yields valid output");
        let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(doc["hookSpecificOutput"]["additionalContext"], "");
        // within the deadline (900ms) + slack — never the 5s busy_timeout.
        assert!(
            elapsed < std::time::Duration::from_secs(3),
            "locked DB stalled the hook for {elapsed:?}"
        );
    }

    /// §P2.2 stdout contract, pure: one JSON doc for claude-code, None for
    /// codex, legacy session_id for hermes/cursor — decided in ONE place.
    #[test]
    fn session_start_stdout_contract_per_tool() {
        let id = Uuid::new_v4();
        let out = session_start_stdout("claude-code", &id, "CTX").unwrap();
        let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(doc["hookSpecificOutput"]["hookEventName"], "SessionStart");
        assert_eq!(doc["hookSpecificOutput"]["additionalContext"], "CTX");
        assert!(!out.contains(&id.to_string()), "session_id stays off stdout");

        assert!(session_start_stdout("codex", &id, "CTX").is_none());

        for tool in ["hermes", "cursor", "some-future-tool"] {
            let out = session_start_stdout(tool, &id, "CTX").unwrap();
            let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
            assert_eq!(doc["session_id"], id.to_string());
        }
    }

    #[test]
    fn guard_json_scrubs_secret_and_pii_in_tool_input() {
        // R11 #2: tool_input/file_changes used to persist RAW. guard_json must
        // scrub every string leaf (secrets + PII) and raise sensitivity.
        let v = serde_json::json!({
            "command": "export OPENAI_API_KEY=sk-ant-AAAAAAAAAAAAAAAAAAAAAAAA",
            "edits": ["contact alice@example.com", 42, null],
        });
        let (scrubbed, count, sens) = guard_json(&v);
        let s = scrubbed.to_string();
        assert!(
            !s.contains("sk-ant-AAAAAAAAAAAAAAAAAAAAAAAA"),
            "secret leaked through tool_input: {s}"
        );
        assert!(!s.contains("alice@example.com"), "email leaked: {s}");
        assert!(count >= 2, "expected ≥2 redacted leaves, got {count}");
        assert!(sens >= altevra_core::security::Sensitivity::Confidential);
        // non-string leaves survive untouched.
        assert!(s.contains("42"));
    }
}

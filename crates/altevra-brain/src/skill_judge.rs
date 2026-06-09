//! P3c — SkillOpt backward pass: success judge + reaction-window drain
//! (PLAN-ALIVE §P3c; Hivemind `success-judge.ts` / `skillopt-improve.ts` port,
//! docs/research/hivemind/01-skillify-engine.md §3.2).
//!
//! Flow: `skill_invocation` events (emitted by hook_handle's PostToolUse on the
//! Skill tool) open a K-message judgment window. The drain (a cheap periodic
//! brain job) pulls pending invocations whose window is satisfied, asks the
//! SUCCESS JUDGE one anti-sycophancy question — *was the skill's guidance
//! CORRECT? Ignore whether the user seemed happy* — and on a judged failure
//! produces a BOUNDED edit proposal routed to the review queue. It **never**
//! publishes/edits a skill file (locked decision: skill edits always go to
//! review).
//!
//! Defensive properties (Hivemind parity):
//!  - **Conservative on failure**: an unreachable model / HTTP error /
//!    unparseable response returns `success = true`. A flaky judge can only
//!    FAIL TO DETECT a deficiency; it can never manufacture one.
//!  - **Meta-fingerprint dedup**: a tried edit set (skillopt_meta.was_tried)
//!    is never re-proposed.
//!  - **Never auto-publish**: failures become `proposals` (kind=skill → Tier-1
//!    → review) + a `review_items` row; no skill file is ever written here.
//!
//! ## Model transport choice (documented per plan)
//!
//! The judge runs on the LOCAL model (lfm2.5:8b via Ollama). lfm2.5 is a heavy
//! `<think>` reasoner — free-form "reply only JSON" prompting FAILS; the
//! live-tested path is Ollama's **native structured-outputs** parameter
//! (`POST /api/generate` with `"format": {json schema}` → valid JSON in ~2s).
//! `altevra_llm::OpenAICompatProvider` has no `response_format` plumbing, so
//! the pragmatic path (per the P3c spec) is a direct Ollama-native call here,
//! with the base URL derived from `[llm].local_private` in
//! `~/.altevra/config.toml` (the `/v1` suffix of the OpenAI-compat endpoint is
//! stripped). The endpoint must be loopback (SI-7: personal-adjacent transcript
//! windows never leave the machine for judging).

use altevra_db::{
    EventsRepository, NewProposal, ProposalsRepository, ReviewItemRow, SessionsRepository,
    SkilloptMetaRepository, TasksRepository, TurnRow,
};
use altevra_skills::skill_edits::{
    apply_edits, fingerprint_edits, SkillEdit, DEFAULT_EDIT_BUDGET,
};
use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use uuid::Uuid;

/// K-message judgment window (Hivemind `DEFAULT_JUDGE_WINDOW = 3`).
pub const REACTION_WINDOW_K: usize = 3;
/// Per-turn char cap inside the judged window (elision, Hivemind ~4000 for the
/// whole window; we cap per turn and total).
pub const WINDOW_TURN_CHAR_CAP: usize = 1200;
/// Total window char cap fed to the judge.
pub const WINDOW_TOTAL_CHAR_CAP: usize = 6000;
/// Max pending invocations drained per job run (keep the job cheap).
const DRAIN_BATCH: i64 = 16;

/// The judge's verdict. `success = true` means "no action" — the conservative
/// default for every failure mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JudgeVerdict {
    pub success: bool,
    #[serde(default)]
    pub weakness: Option<String>,
    #[serde(default)]
    pub confidence: f64,
}

impl JudgeVerdict {
    /// The conservative-on-failure verdict: NOT a failure. A flaky judge can
    /// only miss a deficiency, never manufacture one.
    pub fn conservative() -> Self {
        Self {
            success: true,
            weakness: None,
            confidence: 0.0,
        }
    }
}

/// The JSON schema sent as Ollama's `format` parameter — schema-constrained
/// decoding is what makes lfm2.5 (a `<think>` reasoner) emit valid JSON.
pub fn judge_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "success":    { "type": "boolean" },
            "weakness":   { "type": ["string", "null"] },
            "confidence": { "type": "number" }
        },
        "required": ["success", "weakness", "confidence"]
    })
}

/// The anti-sycophancy judge prompt (Hivemind `success-judge.ts:30` parity).
pub fn judge_prompt(skill: &str, window: &str) -> String {
    format!(
        "You are a strict QA judge for an AI coding assistant.\n\
         The assistant invoked the skill '{skill}' and then the following \
         exchange happened.\n\n\
         Question: was the skill's guidance CORRECT and was the task \
         accomplished correctly?\n\
         IGNORE whether the user seemed happy, satisfied or polite — a \
         praised-but-wrong answer is a FAILURE. A complaint about something \
         unrelated to the skill is NOT a failure.\n\
         If (and only if) the skill's guidance was wrong or insufficient, \
         describe the SINGLE most important weakness in one sentence.\n\n\
         Reply as JSON: {{\"success\": bool, \"weakness\": string|null, \
         \"confidence\": number between 0 and 1}}.\n\n\
         --- EXCHANGE ---\n{window}\n--- END EXCHANGE ---"
    )
}

/// Tolerant verdict parse: strips ``` fences, extracts the first balanced
/// `{...}` object, deserializes. `None` = unparseable (caller goes
/// conservative).
pub fn parse_judge_response(raw: &str) -> Option<JudgeVerdict> {
    let cleaned = raw.trim();
    let cleaned = cleaned
        .strip_prefix("```json")
        .or_else(|| cleaned.strip_prefix("```"))
        .unwrap_or(cleaned);
    let cleaned = cleaned.strip_suffix("```").unwrap_or(cleaned).trim();

    // Fast path: the whole thing is the object (the structured-outputs case).
    if let Ok(v) = serde_json::from_str::<JudgeVerdict>(cleaned) {
        return Some(v);
    }
    // Balanced-brace extraction (Hivemind gate-parser parity) for prose-wrapped
    // output.
    let start = cleaned.find('{')?;
    let bytes = cleaned.as_bytes();
    let mut depth = 0usize;
    let mut in_str = false;
    let mut esc = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if esc {
            esc = false;
            continue;
        }
        match b {
            b'\\' if in_str => esc = true,
            b'"' => in_str = !in_str,
            b'{' if !in_str => depth += 1,
            b'}' if !in_str => {
                depth -= 1;
                if depth == 0 {
                    return serde_json::from_str(&cleaned[start..=i]).ok();
                }
            }
            _ => {}
        }
    }
    None
}

/// The judge contract — async + injectable so tests run with zero HTTP/LLM.
/// Implementations must be infallible: every internal failure maps to
/// [`JudgeVerdict::conservative`].
#[async_trait]
pub trait SuccessJudge: Send + Sync {
    async fn judge(&self, skill: &str, window: &str) -> JudgeVerdict;
}

/// The production judge: Ollama-native structured outputs against the local
/// `[llm].local_private` endpoint (see module docs for why not the
/// OpenAI-compat provider).
pub struct OllamaJudge {
    base_url: String,
    model: String,
    client: reqwest::Client,
}

impl OllamaJudge {
    /// `base_url` is the Ollama ROOT (e.g. `http://localhost:11434`), no `/v1`.
    pub fn new(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            model: model.into(),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .unwrap_or_default(),
        }
    }

    /// Build from `[llm].local_private` in the given config. Returns `None`
    /// when no local provider is configured or the endpoint is not loopback
    /// (SI-7 — the judge never sends transcript windows off-machine).
    pub fn from_llm_config(cfg: &altevra_core::config::LlmConfig) -> Option<Self> {
        let spec = cfg.local_private.as_ref()?;
        let base = spec.base_url.as_deref()?;
        if !url_is_loopback(base) {
            tracing::warn!(
                "skill_judge: local_private endpoint '{base}' is not loopback — refusing (SI-7)"
            );
            return None;
        }
        let model = spec.model.clone()?;
        // The OpenAI-compat endpoint is `<root>/v1`; the native API lives at root.
        let root = base.trim_end_matches('/').trim_end_matches("/v1");
        Some(Self::new(root, model))
    }

    /// Load `~/.altevra/config.toml` and build the judge from its `[llm]`
    /// section. `None` when the file/section/local provider is absent.
    pub fn from_home_config() -> Option<Self> {
        let path = altevra_core::home_dir().join(".altevra/config.toml");
        let content = std::fs::read_to_string(path).ok()?;
        let cfg: altevra_core::config::AltevraConfig = toml::from_str(&content).ok()?;
        Self::from_llm_config(&cfg.llm)
    }
}

fn url_is_loopback(url: &str) -> bool {
    match reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
    {
        Some(h) => {
            let h = h.trim_start_matches('[').trim_end_matches(']');
            matches!(h, "localhost" | "127.0.0.1" | "::1" | "0.0.0.0")
        }
        None => false,
    }
}

#[async_trait]
impl SuccessJudge for OllamaJudge {
    async fn judge(&self, skill: &str, window: &str) -> JudgeVerdict {
        // POST /api/generate with the structured-outputs `format` schema —
        // the live-tested lfm2.5 path. ANY failure → conservative.
        let body = serde_json::json!({
            "model": self.model,
            "prompt": judge_prompt(skill, window),
            "stream": false,
            "format": judge_schema(),
            "options": { "temperature": 0 }
        });
        let resp = match self
            .client
            .post(format!("{}/api/generate", self.base_url))
            .json(&body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("skill_judge: ollama call failed ({e}) — conservative success");
                return JudgeVerdict::conservative();
            }
        };
        if !resp.status().is_success() {
            tracing::warn!(
                "skill_judge: ollama HTTP {} — conservative success",
                resp.status()
            );
            return JudgeVerdict::conservative();
        }
        let text = resp.text().await.unwrap_or_default();
        let answer = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("response").and_then(|r| r.as_str()).map(String::from))
            .unwrap_or_default();
        parse_judge_response(&answer).unwrap_or_else(JudgeVerdict::conservative)
    }
}

/// Pure: extract the judged window text from a session's turns. Includes up to
/// 3 turns of context ENDING at the invocation index, then everything after it
/// until K user "reactions" are consumed. Per-turn + total char caps applied
/// (elision, never raw dumps). The judge runs LOCAL, so this is a privacy-cheap
/// surface — nothing here goes to a cloud.
pub fn extract_reaction_window(turns: &[TurnRow], invocation_turn_idx: i64, k: usize) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut total = 0usize;
    let push = |role: &str, content: &str, lines: &mut Vec<String>, total: &mut usize| {
        if *total >= WINDOW_TOTAL_CHAR_CAP {
            return;
        }
        let trimmed: String = content.chars().take(WINDOW_TURN_CHAR_CAP).collect();
        let suffix = if trimmed.len() < content.len() { "…" } else { "" };
        let line = format!("[{role}] {trimmed}{suffix}");
        *total += line.len();
        lines.push(line);
    };

    // Context: last ≤3 turns up to AND including the invocation turn.
    let before: Vec<&TurnRow> = turns
        .iter()
        .filter(|t| t.turn_idx <= invocation_turn_idx)
        .collect();
    for t in before.iter().rev().take(3).rev() {
        push(&t.role, &t.content, &mut lines, &mut total);
    }

    // Reaction window: after the invocation, until K user turns consumed.
    let mut user_seen = 0usize;
    for t in turns.iter().filter(|t| t.turn_idx > invocation_turn_idx) {
        if user_seen >= k {
            break;
        }
        push(&t.role, &t.content, &mut lines, &mut total);
        if t.role == "user" {
            user_seen += 1;
        }
    }
    lines.join("\n")
}

/// How many user "reaction" turns exist after the invocation index.
fn user_reactions_after(turns: &[TurnRow], invocation_turn_idx: i64) -> usize {
    turns
        .iter()
        .filter(|t| t.turn_idx > invocation_turn_idx && t.role == "user")
        .count()
}

/// Report from one drain pass.
#[derive(Debug, Default, Clone, Serialize)]
pub struct DrainReport {
    pub judged: usize,
    pub failures: usize,
    pub proposals_created: usize,
    pub deferred: usize,
    pub skipped: usize,
}

impl DrainReport {
    pub fn summary(&self) -> String {
        format!(
            "skill_judge: {} judged, {} failures, {} proposals, {} deferred, {} skipped",
            self.judged, self.failures, self.proposals_created, self.deferred, self.skipped
        )
    }
}

/// Drain pending `skill_invocation` events through the success judge.
///
/// `skill_body_for` resolves a skill slug to its current SKILL.md raw content
/// (production: scan of the known skill dirs; tests: a closure over fixtures).
/// On a judged failure: ONE bounded deterministic edit (append a "known
/// weakness" fast-update note) is validated via the P3a `apply_edits` engine,
/// fingerprint-deduped against `skillopt_meta`, and routed to the REVIEW QUEUE
/// (`proposals` kind=skill → Tier-1, plus a `review_items` row). This function
/// NEVER writes a skill file and never auto-applies anything.
pub async fn drain_skill_reactions(
    pool: &SqlitePool,
    judge: &dyn SuccessJudge,
    skill_body_for: &(dyn Fn(&str) -> Option<String> + Sync),
) -> anyhow::Result<DrainReport> {
    use altevra_core::events::{EventStatus, EventType};

    let events = EventsRepository::new(pool);
    let sessions = SessionsRepository::new(pool);
    let mut report = DrainReport::default();

    let pending = events
        .list_pending_by_type(&EventType::SkillInvocation, DRAIN_BATCH)
        .await?;

    for ev in pending {
        let skill = ev
            .payload
            .get("skill")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let session_id = ev
            .payload
            .get("session_id")
            .and_then(serde_json::Value::as_str)
            .and_then(|s| Uuid::parse_str(s).ok());
        let invocation_idx = ev
            .payload
            .get("invocation_turn_idx")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);

        let Some(session_id) = session_id else {
            // Malformed payload — nothing judgeable; close it out.
            let _ = events.mark_status(ev.id, EventStatus::Skipped).await;
            report.skipped += 1;
            continue;
        };

        let session = sessions.get_session(session_id).await?;
        let turns = sessions.list_turns(session_id, 500).await?;
        let reactions = user_reactions_after(&turns, invocation_idx);
        let session_ended = session.as_ref().map(|s| s.ended_at.is_some()).unwrap_or(true);

        // Window readiness: K reactions, or the session ended (judge what we
        // have). An open session with no reaction yet stays PENDING.
        if reactions == 0 {
            if session_ended {
                let _ = events.mark_status(ev.id, EventStatus::Skipped).await;
                report.skipped += 1;
            } else {
                report.deferred += 1;
            }
            continue;
        }
        if reactions < REACTION_WINDOW_K && !session_ended {
            report.deferred += 1;
            continue;
        }

        let window = extract_reaction_window(&turns, invocation_idx, REACTION_WINDOW_K);
        let verdict = judge.judge(&skill, &window).await;
        report.judged += 1;

        if verdict.success {
            let _ = events.mark_status(ev.id, EventStatus::Processed).await;
            continue;
        }
        report.failures += 1;

        let created = propose_bounded_edit(
            pool,
            &skill,
            verdict.weakness.as_deref().unwrap_or("unspecified weakness"),
            &format!("session:{session_id}"),
            &format!("event:{}", ev.id),
            skill_body_for,
        )
        .await?;
        if created {
            report.proposals_created += 1;
        }
        let _ = events.mark_status(ev.id, EventStatus::Processed).await;
    }

    Ok(report)
}

/// Turn ONE judged weakness into a bounded, deterministic edit proposal routed
/// to review. Returns whether a proposal was created (false = deduped/skipped).
///
/// The edit is deliberately LLM-free here (the Codex proposer is the renderer's
/// refine path, `altevra skill-factory render --skill <slug>`): a single
/// `append` fast-update note carrying the judged weakness — the SkillOpt
/// "fast update" that a human reviews before it ever lands.
async fn propose_bounded_edit(
    pool: &SqlitePool,
    skill: &str,
    weakness: &str,
    session_ref: &str,
    event_ref: &str,
    skill_body_for: &(dyn Fn(&str) -> Option<String> + Sync),
) -> anyhow::Result<bool> {
    let Some(body) = skill_body_for(skill) else {
        tracing::info!("skill_judge: no local body for skill '{skill}' — failure recorded, no edit");
        return Ok(false);
    };

    // Guard the weakness text before it lands anywhere durable.
    let guarded =
        altevra_secrets::guard_text(weakness, altevra_core::security::Sensitivity::Internal);
    let weakness = guarded.value;

    let date = Utc::now().format("%Y-%m-%d");
    let edits = vec![SkillEdit::Append {
        text: format!("- Known weakness (skillopt {date}): {weakness}"),
    }];

    // Validate against the CURRENT body via the P3a engine (protected regions
    // + budget). An edit that doesn't change anything proposes nothing.
    let outcome = apply_edits(&body, &edits, DEFAULT_EDIT_BUDGET);
    if !outcome.changed {
        return Ok(false);
    }

    // Meta-fingerprint dedup: never re-propose a tried set.
    let fingerprint = fingerprint_edits(&edits);
    let meta = SkilloptMetaRepository::new(pool);
    if meta.was_tried(skill, &fingerprint).await? {
        return Ok(false);
    }
    let ops: Vec<String> = edits.iter().map(|e| e.summary()).collect();
    meta.record_tried(skill, &fingerprint, &serde_json::json!(ops), "proposed")
        .await?;

    // Review queue, never auto-publish: proposals row (kind=skill → Tier-1 by
    // derive_risk_tier — auto-apply firewalled) + a review_items row.
    let proposal_body = serde_json::json!({
        "skill": skill,
        "weakness": weakness,
        "edits": edits,
        "fingerprint": fingerprint,
        "engine": "skillopt-backward-pass-v1",
    });
    let proposals = ProposalsRepository::new(pool);
    let (proposal_id, is_new) = proposals
        .insert(&NewProposal {
            kind: "skill".into(),
            title: format!("skillopt: '{skill}' judged failure — bounded edit"),
            body: proposal_body.to_string(),
            source_mode: Some("skill_reaction_judge".into()),
            dedup_hash: format!("skillopt:{skill}:{fingerprint}"),
            evidence_refs: vec![session_ref.to_string(), event_ref.to_string()],
            touches_sensitive: false,
            touches_constitutional: false,
        })
        .await?;
    if !is_new {
        return Ok(false);
    }

    let review = ReviewItemRow {
        id: Uuid::new_v4(),
        project_id: None,
        kind: "skill_edit".into(),
        title: format!("Review skillopt edit for '{skill}'"),
        body: Some(format!(
            "Judged weakness: {weakness}\n\nProposed edits (P3a SkillEdit JSON):\n{}",
            serde_json::to_string_pretty(&edits)?
        )),
        status: "open".into(),
        created_at: Utc::now(),
        metadata: serde_json::json!({
            "proposal_id": proposal_id,
            "skill": skill,
            "fingerprint": fingerprint,
        }),
    };
    TasksRepository::new(pool).create_review_item(&review).await?;
    Ok(true)
}

/// Production skill-body resolver: scan the known skill dirs for `slug`.
pub fn default_skill_body_for(slug: &str) -> Option<String> {
    for s in altevra_skills::importer::scan_all() {
        if s.slug == slug {
            return std::fs::read_to_string(&s.path).ok();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use altevra_db::{create_pool, run_migrations, SessionRow};
    use chrono::Utc;

    async fn test_pool(dir: &tempfile::TempDir) -> SqlitePool {
        let db = dir.path().join("judge.db");
        let p = create_pool(&db.to_string_lossy()).await.unwrap();
        run_migrations(&p).await.unwrap();
        p
    }

    fn turn(session_id: Uuid, idx: i64, role: &str, content: &str) -> TurnRow {
        TurnRow {
            id: Uuid::new_v4(),
            session_id,
            turn_idx: idx,
            role: role.into(),
            content: content.into(),
            tool_calls: None,
            tool_name: None,
            model: None,
            tokens_in: None,
            tokens_out: None,
            latency_ms: None,
            file_changes: None,
            redacted_count: 0,
            source_tool: Some("claude-code".into()),
            sensitivity: "internal".into(),
            redaction_status: "clean".into(),
            created_at: Utc::now(),
            working_dir: None,
        }
    }

    async fn seed_session_with_turns(
        pool: &SqlitePool,
        turns: &[(i64, &str, &str)],
        ended: bool,
    ) -> Uuid {
        let repo = SessionsRepository::new(pool);
        let id = Uuid::new_v4();
        repo.start_session(&SessionRow {
            id,
            tool: "claude-code".into(),
            project_id: None,
            project_name: Some("altevra".into()),
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
            working_dir: Some("/home/x/proj".into()),
        })
        .await
        .unwrap();
        for (idx, role, content) in turns {
            repo.record_turn(&turn(id, *idx, role, content)).await.unwrap();
        }
        if ended {
            repo.end_session(id, None).await.unwrap();
        }
        id
    }

    async fn seed_invocation(pool: &SqlitePool, session_id: Uuid, skill: &str, idx: i64) -> Uuid {
        use altevra_core::events::{ActorType, Event, EventType};
        let ev = Event::new(
            EventType::SkillInvocation,
            "test invocation",
            "test",
            ActorType::Agent,
        )
        .with_entity("session", session_id.to_string())
        .with_payload(serde_json::json!({
            "skill": skill,
            "session_id": session_id.to_string(),
            "invocation_turn_idx": idx,
        }));
        EventsRepository::new(pool).insert(&ev).await.unwrap();
        ev.id
    }

    struct FixedJudge(JudgeVerdict);
    #[async_trait]
    impl SuccessJudge for FixedJudge {
        async fn judge(&self, _skill: &str, _window: &str) -> JudgeVerdict {
            self.0.clone()
        }
    }

    // ---------- verdict parsing ----------

    #[test]
    fn parse_valid_structured_response() {
        let v = parse_judge_response(
            r#"{"success": false, "weakness": "ignores --apply flag", "confidence": 0.9}"#,
        )
        .unwrap();
        assert!(!v.success);
        assert_eq!(v.weakness.as_deref(), Some("ignores --apply flag"));
        assert!((v.confidence - 0.9).abs() < 1e-9);
    }

    #[test]
    fn parse_fenced_and_prose_wrapped_responses() {
        let fenced = "```json\n{\"success\": true, \"weakness\": null, \"confidence\": 1.0}\n```";
        assert!(parse_judge_response(fenced).unwrap().success);

        let prose = "Sure! Here is my judgment: {\"success\": false, \"weakness\": \"bad anchor\", \"confidence\": 0.5} hope that helps";
        let v = parse_judge_response(prose).unwrap();
        assert!(!v.success);
        assert_eq!(v.weakness.as_deref(), Some("bad anchor"));
    }

    #[test]
    fn unparseable_response_is_none_and_callers_go_conservative() {
        assert!(parse_judge_response("<think>hmm I wonder</think> not json").is_none());
        assert!(parse_judge_response("").is_none());
        // The conservative verdict is success=true: a flaky judge can never
        // manufacture deficiency.
        assert!(JudgeVerdict::conservative().success);
    }

    // ---------- conservative-on-failure over real (mocked) HTTP ----------

    /// Minimal one-shot HTTP server: accepts a single connection, replies with
    /// the canned body. No mock-server dependency needed.
    async fn one_shot_http(body: String) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = vec![0u8; 65536];
                let _ = sock.read(&mut buf).await;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.shutdown().await;
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn ollama_judge_parses_valid_structured_output() {
        // The Ollama native response wraps the model output in `response`.
        let inner = r#"{"success": false, "weakness": "step 3 is wrong", "confidence": 0.8}"#;
        let body = serde_json::json!({ "response": inner }).to_string();
        let base = one_shot_http(body).await;
        let judge = OllamaJudge::new(base, "lfm2.5:8b");
        let v = judge.judge("my-skill", "[user] it broke").await;
        assert!(!v.success);
        assert_eq!(v.weakness.as_deref(), Some("step 3 is wrong"));
    }

    #[tokio::test]
    async fn ollama_judge_is_conservative_on_garbage_and_on_dead_endpoint() {
        // Garbage model output → conservative success.
        let body = serde_json::json!({ "response": "<think>…</think> no json here" }).to_string();
        let base = one_shot_http(body).await;
        let v = OllamaJudge::new(base, "m").judge("s", "w").await;
        assert!(v.success, "unparseable judgment must NOT be a failure");

        // Dead endpoint (nothing listening) → conservative success, no error.
        let v = OllamaJudge::new("http://127.0.0.1:1", "m").judge("s", "w").await;
        assert!(v.success, "errored judgment must NOT be a failure");
    }

    #[test]
    fn from_llm_config_requires_loopback_and_strips_v1() {
        use altevra_core::config::{LlmConfig, ProviderSettings};
        let cfg = LlmConfig {
            local_private: Some(ProviderSettings {
                kind: Some("openai_compat".into()),
                base_url: Some("http://localhost:11434/v1".into()),
                model: Some("lfm2.5:8b".into()),
                secret_key: None,
            }),
            ..Default::default()
        };
        let j = OllamaJudge::from_llm_config(&cfg).unwrap();
        assert_eq!(j.base_url, "http://localhost:11434");
        assert_eq!(j.model, "lfm2.5:8b");

        // Non-loopback endpoint → refused (SI-7).
        let cfg = LlmConfig {
            local_private: Some(ProviderSettings {
                kind: Some("openai_compat".into()),
                base_url: Some("https://api.example.com/v1".into()),
                model: Some("x".into()),
                secret_key: None,
            }),
            ..Default::default()
        };
        assert!(OllamaJudge::from_llm_config(&cfg).is_none());
    }

    // ---------- reaction-window extraction ----------

    #[test]
    fn reaction_window_takes_context_and_k_user_turns() {
        let sid = Uuid::new_v4();
        let turns = vec![
            turn(sid, 0, "user", "please run the skill"),
            turn(sid, 1, "tool_result", "skill output"),
            turn(sid, 2, "user", "reaction one"),
            turn(sid, 3, "assistant", "assistant reply"),
            turn(sid, 4, "user", "reaction two"),
            turn(sid, 5, "user", "reaction three"),
            turn(sid, 6, "user", "PAST THE WINDOW"),
        ];
        let w = extract_reaction_window(&turns, 1, REACTION_WINDOW_K);
        assert!(w.contains("please run the skill"), "context before invocation");
        assert!(w.contains("skill output"));
        assert!(w.contains("reaction one"));
        assert!(w.contains("assistant reply"), "interleaved turns included");
        assert!(w.contains("reaction three"));
        assert!(!w.contains("PAST THE WINDOW"), "window stops at K user turns");
    }

    #[test]
    fn reaction_window_caps_chars() {
        let sid = Uuid::new_v4();
        let huge = "x".repeat(50_000);
        let turns = vec![turn(sid, 0, "tool_result", &huge), turn(sid, 1, "user", &huge)];
        let w = extract_reaction_window(&turns, 0, REACTION_WINDOW_K);
        assert!(w.len() <= WINDOW_TOTAL_CHAR_CAP + WINDOW_TURN_CHAR_CAP + 64);
        assert!(w.contains('…'), "elision marker present");
    }

    // ---------- drain: review-queue routing, never auto-publish ----------

    const SKILL_BODY: &str = "---\nslug: my-skill\nversion: 1.0.0\ntitle: My Skill\n---\n# My Skill\n\n## Usage\nrun it\n";

    #[tokio::test]
    async fn judged_failure_routes_to_review_queue_never_publishes() {
        let dir = tempfile::TempDir::new().unwrap();
        let pool = test_pool(&dir).await;
        // Session with K reactions after the invocation at idx 1.
        let sid = seed_session_with_turns(
            &pool,
            &[
                (0, "user", "use the skill"),
                (1, "tool_result", "skill ran"),
                (2, "user", "that's wrong"),
                (3, "user", "still wrong"),
                (4, "user", "broken"),
            ],
            false,
        )
        .await;
        seed_invocation(&pool, sid, "my-skill", 1).await;

        // A skill "file" on disk that must NEVER be modified by the drain.
        let skill_path = dir.path().join("SKILL.md");
        std::fs::write(&skill_path, SKILL_BODY).unwrap();

        let judge = FixedJudge(JudgeVerdict {
            success: false,
            weakness: Some("ignores the --apply flag".into()),
            confidence: 0.9,
        });
        let body_for = |slug: &str| {
            if slug == "my-skill" {
                Some(SKILL_BODY.to_string())
            } else {
                None
            }
        };
        let report = drain_skill_reactions(&pool, &judge, &body_for).await.unwrap();
        assert_eq!(report.judged, 1);
        assert_eq!(report.failures, 1);
        assert_eq!(report.proposals_created, 1);

        // Proposal exists, kind=skill, Tier-1 (review-gated), status proposed.
        let proposals = ProposalsRepository::new(&pool)
            .list(Some("proposed"), Some("skill"))
            .await
            .unwrap();
        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals[0].risk_tier, "tier1", "skill edits are never Tier-0");
        let body: serde_json::Value = serde_json::from_str(&proposals[0].body).unwrap();
        assert!(body["weakness"].as_str().unwrap().contains("--apply"));
        assert!(body["edits"].is_array());

        // Review item exists.
        let reviews = TasksRepository::new(&pool)
            .list_review_items(Some("open"), 10)
            .await
            .unwrap();
        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0].kind, "skill_edit");

        // skillopt_meta fingerprint recorded as 'proposed' — NEVER 'applied'.
        let meta = SkilloptMetaRepository::new(&pool)
            .list_for_skill("my-skill")
            .await
            .unwrap();
        assert_eq!(meta.len(), 1);
        assert_eq!(meta[0].outcome, "proposed");

        // THE never-auto-publish assertion: the skill file is byte-identical.
        assert_eq!(std::fs::read_to_string(&skill_path).unwrap(), SKILL_BODY);

        // Event marked processed: a re-drain judges nothing.
        let again = drain_skill_reactions(&pool, &judge, &body_for).await.unwrap();
        assert_eq!(again.judged, 0);
    }

    #[tokio::test]
    async fn judged_success_marks_processed_with_no_proposal() {
        let dir = tempfile::TempDir::new().unwrap();
        let pool = test_pool(&dir).await;
        let sid = seed_session_with_turns(
            &pool,
            &[
                (0, "tool_result", "skill ran"),
                (1, "user", "perfect"),
                (2, "user", "thanks"),
                (3, "user", "done"),
            ],
            false,
        )
        .await;
        seed_invocation(&pool, sid, "my-skill", 0).await;

        let judge = FixedJudge(JudgeVerdict {
            success: true,
            weakness: None,
            confidence: 1.0,
        });
        let body_for = |_: &str| Some(SKILL_BODY.to_string());
        let report = drain_skill_reactions(&pool, &judge, &body_for).await.unwrap();
        assert_eq!(report.judged, 1);
        assert_eq!(report.failures, 0);
        assert!(ProposalsRepository::new(&pool)
            .list(None, Some("skill"))
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn open_session_with_unspent_window_is_deferred() {
        let dir = tempfile::TempDir::new().unwrap();
        let pool = test_pool(&dir).await;
        // Only ONE reaction so far; session still open → defer (stay pending).
        let sid = seed_session_with_turns(
            &pool,
            &[(0, "tool_result", "skill ran"), (1, "user", "hmm")],
            false,
        )
        .await;
        seed_invocation(&pool, sid, "my-skill", 0).await;

        let judge = FixedJudge(JudgeVerdict::conservative());
        let body_for = |_: &str| Some(SKILL_BODY.to_string());
        let report = drain_skill_reactions(&pool, &judge, &body_for).await.unwrap();
        assert_eq!(report.judged, 0);
        assert_eq!(report.deferred, 1);

        // Once the session ends, the partial window IS judged.
        SessionsRepository::new(&pool).end_session(sid, None).await.unwrap();
        let report = drain_skill_reactions(&pool, &judge, &body_for).await.unwrap();
        assert_eq!(report.judged, 1);
    }

    #[tokio::test]
    async fn tried_fingerprint_is_never_reproposed() {
        let dir = tempfile::TempDir::new().unwrap();
        let pool = test_pool(&dir).await;
        let sid = seed_session_with_turns(
            &pool,
            &[
                (0, "tool_result", "skill ran"),
                (1, "user", "bad"),
                (2, "user", "bad"),
                (3, "user", "bad"),
            ],
            true,
        )
        .await;
        seed_invocation(&pool, sid, "my-skill", 0).await;

        let judge = FixedJudge(JudgeVerdict {
            success: false,
            weakness: Some("same weakness".into()),
            confidence: 0.9,
        });
        let body_for = |_: &str| Some(SKILL_BODY.to_string());

        // Pre-record the EXACT edit set the drain would propose (same date-less
        // shape is hard to predict, so run once, then re-seed a second
        // invocation and assert no second proposal/meta row).
        drain_skill_reactions(&pool, &judge, &body_for).await.unwrap();
        assert_eq!(
            SkilloptMetaRepository::new(&pool).list_for_skill("my-skill").await.unwrap().len(),
            1
        );

        seed_invocation(&pool, sid, "my-skill", 0).await;
        let report = drain_skill_reactions(&pool, &judge, &body_for).await.unwrap();
        assert_eq!(report.failures, 1, "failure judged again");
        assert_eq!(report.proposals_created, 0, "tried fingerprint never re-proposed");
        assert_eq!(
            SkilloptMetaRepository::new(&pool).list_for_skill("my-skill").await.unwrap().len(),
            1,
            "no duplicate meta row"
        );
        assert_eq!(
            ProposalsRepository::new(&pool).list(None, Some("skill")).await.unwrap().len(),
            1,
            "still exactly one proposal"
        );
    }
}

//! R4 integration tests — observer detectors + events retention + judge-drain
//! connectivity.
//!
//! Four coverage areas per the PLAN-ROUND.md §R4 gate spec:
//!
//!   1. **Per-detector fixture firing + silence-below-threshold** — covered
//!      by the unit tests inside `observer_detectors.rs`. This file adds
//!      the cross-crate integration layer (real pool, real repos, brain-job
//!      wiring).
//!
//!   2. **Metadata-only event emission** — new events produced by the DB
//!      detectors carry turn-id refs + counts, NEVER payload copies.
//!
//!   3. **Retention prune** — the lifecycle_archiver brain job prunes
//!      noise-class events older than the configured window while keeping
//!      durable events.
//!
//!   4. **skill_invocation → skill_reaction → judge drain** — end-to-end
//!      on fixtures: seed an invocation event + session turns, run the
//!      drain, verify the proposal appears in the review queue.

use altevra_brain::{
    observer_detectors::{prune_noise_events, run_db_detectors, DEFAULT_RETENTION_DAYS},
    skill_judge::{drain_skill_reactions, JudgeVerdict, SuccessJudge},
};
use altevra_db::{
    EventsRepository, ProposalsRepository, SessionRow, SessionsRepository, TurnRow,
};
use async_trait::async_trait;
use chrono::{Duration, Utc};
use sqlx::SqlitePool;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn pool() -> SqlitePool {
    let p = altevra_db::create_pool("sqlite::memory:").await.unwrap();
    altevra_db::run_migrations(&p).await.unwrap();
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
        working_dir: Some("/home/pavle/proj".into()),
    }
}

async fn seed_session_with_turns(
    pool: &SqlitePool,
    turns_data: &[(i64, &str, &str)],
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
        working_dir: Some("/home/pavle/proj/altevra".into()),
    })
    .await
    .unwrap();
    for (idx, role, content) in turns_data {
        repo.record_turn(&turn(id, *idx, role, content)).await.unwrap();
    }
    if ended {
        repo.end_session(id, None).await.unwrap();
    }
    id
}

async fn seed_invocation(pool: &SqlitePool, session_id: Uuid, skill: &str, idx: i64) {
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
}

struct FixedJudge(JudgeVerdict);
#[async_trait]
impl SuccessJudge for FixedJudge {
    async fn judge(&self, _skill: &str, _window: &str) -> JudgeVerdict {
        self.0.clone()
    }
}

const SKILL_BODY: &str =
    "---\nslug: test-skill\nversion: 1.0.0\ntitle: Test Skill\n---\n# Test Skill\n\n## Usage\nrun it\n";

// ---------------------------------------------------------------------------
// Test 1: metadata-only emission invariant (cross-crate)
// ---------------------------------------------------------------------------

/// New events emitted by the DB-backed observer detectors carry turn-id refs
/// and counts — NEVER the payload / body of any turn.
#[tokio::test]
async fn metadata_only_event_emission_via_proposals() {
    let p = pool().await;
    let now = Utc::now();

    // Seed 3 distinct working dirs → working_dir_drift insight.
    for (i, dir) in ["/proj/alpha", "/proj/beta", "/proj/gamma"].iter().enumerate() {
        sqlx::query(
            r#"INSERT INTO sessions
               (id, tool, started_at, working_dir, project_name,
                tokens_in_total, tokens_out_total, cost_usd_estimate, turn_count, metadata)
               VALUES (?, 'claude-code', ?, ?, 'T', 0, 0, 0.0, 0, '{}')"#,
        )
        .bind(format!("s-emit-{i}"))
        .bind((now - Duration::hours(i as i64 + 1)).to_rfc3339())
        .bind(dir)
        .execute(&p)
        .await
        .unwrap();
    }

    let insights = run_db_detectors(&p, now).await.unwrap();
    assert!(!insights.is_empty(), "should produce ≥1 insight");

    // Each evidence item must be metadata-only.
    for insight in &insights {
        for ev in &insight.evidence {
            // Labels must not carry body text (> 300 chars would be suspicious).
            assert!(
                ev.label.len() < 300,
                "evidence label too long ({} chars) — likely contains body: '{}'",
                ev.label.len(),
                &ev.label[..ev.label.len().min(100)]
            );
            // IDs must be short identifiers (UUIDs, slugs, paths), not blobs.
            if let Some(id) = &ev.id {
                assert!(
                    id.len() < 512,
                    "evidence id too long ({} chars) — possible body leak",
                    id.len()
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Test 2: retention prune (integration layer)
// ---------------------------------------------------------------------------

/// The events retention sweep prunes noise-class events older than the window
/// while leaving durable (skill / session / decision) events intact.
#[tokio::test]
async fn retention_prune_integration() {
    use altevra_core::events::{ActorType, Event, EventType};

    let p = pool().await;
    let now = Utc::now();
    let repo = EventsRepository::new(&p);

    let old = now - Duration::days(DEFAULT_RETENTION_DAYS + 2);

    // Old noise events (should be pruned).
    let noise = [
        EventType::ToolCallObserved,
        EventType::FileChanged,
        EventType::McpCall,
    ];
    for nt in &noise {
        let mut ev = Event::new(nt.clone(), "noise", "test", ActorType::System);
        ev.created_at = old;
        repo.insert(&ev).await.unwrap();
    }

    // Old durable events (must survive).
    let durable = [
        EventType::SkillDriftDetected,
        EventType::SessionStarted,
        EventType::DecisionSaved,
    ];
    for dt in &durable {
        let mut ev = Event::new(dt.clone(), "durable", "test", ActorType::System);
        ev.created_at = old;
        repo.insert(&ev).await.unwrap();
    }

    let report = prune_noise_events(&p, now, DEFAULT_RETENTION_DAYS).await.unwrap();
    assert_eq!(report.pruned, noise.len(), "all old noise events must be pruned");

    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE title = 'durable'")
        .fetch_one(&p)
        .await
        .unwrap();
    assert_eq!(
        remaining,
        durable.len() as i64,
        "durable events must survive retention"
    );
}

// ---------------------------------------------------------------------------
// Test 3: skill_invocation → skill_reaction → judge drain (end-to-end)
// ---------------------------------------------------------------------------

/// Verify the full P3c path: seed a SkillInvocation event + session turns →
/// run drain_skill_reactions → proposal appears in the review queue.
///
/// This is the "judge-drain connectivity" test mandated by PLAN-ROUND.md §R4.
#[tokio::test]
async fn skill_invocation_reaction_judge_drain_end_to_end() {
    let p = pool().await;

    // Seed a session with a skill invocation at turn-idx 1 and K user reactions.
    let sid = seed_session_with_turns(
        &p,
        &[
            (0, "user", "please run the skill"),
            (1, "tool_result", "skill output here"),
            (2, "user", "that was wrong"),
            (3, "user", "still broken"),
            (4, "user", "not fixed"),
        ],
        false, // still open
    )
    .await;
    seed_invocation(&p, sid, "test-skill", 1).await;

    // Judge that always fails → triggers the proposal path.
    let judge = FixedJudge(JudgeVerdict {
        success: false,
        weakness: Some("output was incorrect".into()),
        confidence: 0.9,
    });
    let body_for = |slug: &str| {
        if slug == "test-skill" {
            Some(SKILL_BODY.to_string())
        } else {
            None
        }
    };

    let report = drain_skill_reactions(&p, &judge, &body_for).await.unwrap();

    // Window is satisfied (K=3 user reactions) even though session is open.
    assert_eq!(report.judged, 1, "one invocation must be judged");
    assert_eq!(report.failures, 1, "the fixed judge returns failure");
    assert_eq!(report.proposals_created, 1, "one proposal must be created");
    assert_eq!(report.deferred, 0, "K reactions reached → not deferred");

    // The proposal must be in the review queue (kind=skill, status=proposed).
    let proposals = ProposalsRepository::new(&p)
        .list(Some("proposed"), Some("skill"))
        .await
        .unwrap();
    assert_eq!(proposals.len(), 1, "exactly one skill proposal in the queue");
    assert_eq!(
        proposals[0].risk_tier, "tier1",
        "skill edits are review-gated (never Tier-0)"
    );

    // The SkillInvocation event must be marked processed → a second drain is a no-op.
    let again = drain_skill_reactions(&p, &judge, &body_for).await.unwrap();
    assert_eq!(again.judged, 0, "re-drain must find nothing pending");
    assert_eq!(again.proposals_created, 0, "no duplicate proposals");
}

/// Verify the drain is SILENT (conservative) when the judge succeeds — no
/// proposal is created and the event is marked processed.
#[tokio::test]
async fn skill_invocation_success_path_no_proposal() {
    let p = pool().await;

    let sid = seed_session_with_turns(
        &p,
        &[
            (0, "tool_result", "skill output"),
            (1, "user", "great, thanks"),
            (2, "user", "exactly what I needed"),
            (3, "user", "done"),
        ],
        false,
    )
    .await;
    seed_invocation(&p, sid, "test-skill", 0).await;

    let judge = FixedJudge(JudgeVerdict {
        success: true,
        weakness: None,
        confidence: 1.0,
    });
    let body_for = |_: &str| Some(SKILL_BODY.to_string());
    let report = drain_skill_reactions(&p, &judge, &body_for).await.unwrap();

    assert_eq!(report.judged, 1);
    assert_eq!(report.failures, 0);
    assert_eq!(report.proposals_created, 0, "success → no proposal");

    let proposals = ProposalsRepository::new(&p)
        .list(None, Some("skill"))
        .await
        .unwrap();
    assert!(proposals.is_empty(), "success path must not create proposals");
}

/// Verify that a session with fewer than K reactions and still open is
/// deferred (not judged) — the window isn't ready yet.
#[tokio::test]
async fn skill_invocation_deferred_when_window_not_ready() {
    let p = pool().await;

    // Only 1 reaction; K=3; session still open → deferred.
    let sid = seed_session_with_turns(
        &p,
        &[
            (0, "tool_result", "skill output"),
            (1, "user", "one reaction so far"),
        ],
        false,
    )
    .await;
    seed_invocation(&p, sid, "test-skill", 0).await;

    let judge = FixedJudge(JudgeVerdict::conservative());
    let body_for = |_: &str| Some(SKILL_BODY.to_string());
    let report = drain_skill_reactions(&p, &judge, &body_for).await.unwrap();

    assert_eq!(report.judged, 0, "must not judge when window not ready");
    assert_eq!(report.deferred, 1, "must be deferred (window not satisfied)");

    // Once the session ends, the partial window is consumed.
    SessionsRepository::new(&p).end_session(sid, None).await.unwrap();
    let report2 = drain_skill_reactions(&p, &judge, &body_for).await.unwrap();
    assert_eq!(report2.judged, 1, "ended session forces judgment on partial window");
}

// ---------------------------------------------------------------------------
// Test 4: run_observer_scan wires DB detectors + produces proposals
// ---------------------------------------------------------------------------

/// The `run_observer_scan` brain job now runs both event-pattern detectors
/// AND DB-backed detectors. Seed sessions that trigger a DB detector and
/// verify a proposal is persisted.
#[tokio::test]
async fn observer_scan_job_wires_db_detectors() {
    use altevra_brain::jobs::JobContext;
    use altevra_db::ProposalsRepository;

    let p = pool().await;
    let now = Utc::now();

    // Seed 3 distinct working dirs → working_dir_drift DB insight.
    for (i, dir) in ["/proj/a", "/proj/b", "/proj/c"].iter().enumerate() {
        sqlx::query(
            r#"INSERT INTO sessions
               (id, tool, started_at, working_dir, project_name,
                tokens_in_total, tokens_out_total, cost_usd_estimate, turn_count, metadata)
               VALUES (?, 'claude-code', ?, ?, 'T', 0, 0, 0.0, 0, '{}')"#,
        )
        .bind(format!("s-scan-{i}"))
        .bind((now - Duration::hours(i as i64 + 1)).to_rfc3339())
        .bind(dir)
        .execute(&p)
        .await
        .unwrap();
    }

    let tmp = tempfile::tempdir().unwrap();
    let ctx = JobContext {
        vault_path: tmp.path().to_path_buf(),
        now,
        router: std::sync::Arc::new(altevra_llm::ModelRouter::noop()),
    };

    // Run only the observer scan job.
    let result = altevra_brain::jobs::run_observer_scan(&p, &ctx).await.unwrap();

    // Should have processed ≥1 insight (the DB drift detector fired).
    assert!(
        result.items_processed >= 1,
        "observer scan must detect the working_dir_drift insight; got: {:?}",
        result
    );

    // The insight must be persisted as a proposal.
    let proposals = ProposalsRepository::new(&p).list(None, None).await.unwrap();
    assert!(
        proposals.iter().any(|p| p.source_mode.as_deref() == Some("observer_db")),
        "at least one proposal must come from the db-detector path"
    );
}

// ---------------------------------------------------------------------------
// Test 5: lifecycle_archiver job includes retention prune
// ---------------------------------------------------------------------------

/// The lifecycle_archiver brain job now also runs the events retention sweep.
/// Verify that after the job runs, old noise events are gone.
#[tokio::test]
async fn lifecycle_archiver_includes_events_retention() {
    use altevra_brain::jobs::JobContext;
    use altevra_core::events::{ActorType, Event, EventType};

    let p = pool().await;
    let now = Utc::now();
    let old = now - Duration::days(DEFAULT_RETENTION_DAYS + 5);
    let repo = EventsRepository::new(&p);

    // Seed old noise events.
    for nt in [EventType::ToolCallObserved, EventType::FileChanged] {
        let mut ev = Event::new(nt, "noise", "test", ActorType::System);
        ev.created_at = old;
        repo.insert(&ev).await.unwrap();
    }
    // Seed durable event (must survive).
    let mut durable = Event::new(EventType::DecisionSaved, "durable", "test", ActorType::User);
    durable.created_at = old;
    repo.insert(&durable).await.unwrap();

    let before_noise: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE title = 'noise'")
        .fetch_one(&p)
        .await
        .unwrap();
    assert_eq!(before_noise, 2, "pre-condition: 2 noise events");

    let tmp = tempfile::tempdir().unwrap();
    let ctx = JobContext {
        vault_path: tmp.path().to_path_buf(),
        now,
        router: std::sync::Arc::new(altevra_llm::ModelRouter::noop()),
    };

    altevra_brain::jobs::run_lifecycle_archiver(&p, &ctx).await.unwrap();

    let after_noise: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE title = 'noise'")
        .fetch_one(&p)
        .await
        .unwrap();
    assert_eq!(after_noise, 0, "noise events must be pruned by the lifecycle archiver");

    let after_durable: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE title = 'durable'")
            .fetch_one(&p)
            .await
            .unwrap();
    assert_eq!(after_durable, 1, "durable event must survive the prune");
}

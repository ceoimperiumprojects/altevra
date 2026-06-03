//! Round-trip integration tests for every repository, exercised against a
//! fresh in-memory SQLite database. Migrations are applied at the top of each
//! test via the public `run_migrations` API so we exercise the full SQL
//! surface, not handcrafted schemas.
//!
//! These tests double as the executable contract for downstream crates: if a
//! repository signature changes, the relevant test breaks immediately.

use altevra_core::events::{ActorType, Event, EventStatus, EventType};
use altevra_core::security::Sensitivity;
use altevra_core::updates::{Importance, UpdateFeedItem, UpdatesQuery};
use altevra_db::{
    create_pool, run_migrations, DecisionIndexEnvelope, DecisionRow, EventsRepository,
    ExposureAudit, ExposureDecisionsRepository, FtsRepository, GoalRow, HookRow, HookRunRow,
    HooksRepository, InstallationsRepository, InstalledComponentRow, ObjectIndexRepository,
    ReadStateRepository, ReviewItemRow, SkillRow, SkillsRepository, TaskRow, TasksRepository,
    ToolInstallationRow, UpdatesRepository, WikiPagesRepository,
};
use chrono::{NaiveDate, Utc};
use uuid::Uuid;

async fn fresh_pool() -> altevra_db::DbPool {
    let pool = create_pool("sqlite::memory:")
        .await
        .expect("memory pool should always be creatable");
    run_migrations(&pool)
        .await
        .expect("migrations should apply cleanly to a fresh sqlite db");
    pool
}

#[tokio::test]
async fn events_roundtrip() {
    let pool = fresh_pool().await;
    let repo = EventsRepository::new(&pool);

    let evt = Event::new(
        EventType::TaskCreated,
        "Roundtrip event",
        "test::events",
        ActorType::System,
    )
    .with_summary("smoke");
    repo.insert(&evt).await.unwrap();

    let listed = repo
        .list_since(Utc::now() - chrono::Duration::days(1), None, 10)
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    let got = &listed[0];
    assert_eq!(got.id, evt.id);
    assert_eq!(got.title, "Roundtrip event");
    assert_eq!(got.event_type, EventType::TaskCreated);
    assert_eq!(got.actor_type, ActorType::System);
    assert_eq!(got.status, EventStatus::Pending);
    assert_eq!(got.sensitivity, Sensitivity::Internal);
}

#[tokio::test]
async fn updates_roundtrip() {
    let pool = fresh_pool().await;

    // Updates reference an event via FK, so seed one.
    let events = EventsRepository::new(&pool);
    let evt = Event::new(
        EventType::DocumentChanged,
        "Doc changed",
        "test::updates",
        ActorType::System,
    );
    events.insert(&evt).await.unwrap();

    let repo = UpdatesRepository::new(&pool);
    let item = UpdateFeedItem::from_event(
        evt.id,
        "doc.changed",
        Importance::High,
        "Doc changed",
        "/foo/bar.md",
    );
    repo.insert(&item).await.unwrap();

    let listed = repo
        .query(&UpdatesQuery {
            since: Some(Utc::now() - chrono::Duration::hours(1)),
            limit: Some(10),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, item.id);
    assert_eq!(listed[0].importance, Importance::High);
    assert!(listed[0].visible_to_agents);

    let last = repo.get_last_n(5).await.unwrap();
    assert_eq!(last.len(), 1);
}

#[tokio::test]
async fn skills_roundtrip() {
    let pool = fresh_pool().await;
    let repo = SkillsRepository::new(&pool);

    let now = Utc::now();
    let row = SkillRow {
        id: Uuid::new_v4(),
        slug: "code-review".to_string(),
        version: "1.2.3".to_string(),
        source_path: "/skills/code-review.md".to_string(),
        checksum: "deadbeef".to_string(),
        content: "# Code review skill".to_string(),
        metadata: serde_json::json!({"tags": ["review", "qa"]}),
        status: "active".to_string(),
        created_at: now,
        updated_at: now,
    };
    repo.upsert(&row).await.unwrap();

    // Upsert again with a different version — should update in place.
    let mut row2 = row.clone();
    row2.version = "1.2.4".to_string();
    row2.updated_at = now + chrono::Duration::seconds(1);
    repo.upsert(&row2).await.unwrap();

    let fetched = repo.find_by_slug("code-review").await.unwrap().unwrap();
    assert_eq!(fetched.version, "1.2.4");
    assert_eq!(fetched.metadata["tags"][0], "review");

    let all = repo.list_all().await.unwrap();
    assert_eq!(all.len(), 1);
}

#[tokio::test]
async fn hooks_roundtrip() {
    let pool = fresh_pool().await;
    let repo = HooksRepository::new(&pool);

    let now = Utc::now();
    let hook = HookRow {
        id: Uuid::new_v4(),
        slug: "post-edit".to_string(),
        version: "0.1.0".to_string(),
        source_file: "/hooks/post-edit.sh".to_string(),
        checksum: "abc123".to_string(),
        status: "active".to_string(),
        created_at: now,
        updated_at: now,
    };
    repo.upsert_hook(&hook).await.unwrap();
    let hooks = repo.list_hooks().await.unwrap();
    assert_eq!(hooks.len(), 1);
    assert_eq!(hooks[0].slug, "post-edit");

    let run = HookRunRow {
        id: Uuid::new_v4(),
        hook_slug: "post-edit".to_string(),
        tool_name: "claude".to_string(),
        project_id: None,
        payload: serde_json::json!({"file": "/tmp/foo"}),
        result: serde_json::json!({"ok": true}),
        success: true,
        error_message: None,
        duration_ms: 42,
        created_at: now,
    };
    repo.log_run(&run).await.unwrap();
    let recent = repo.get_recent_runs("post-edit", 5).await.unwrap();
    assert_eq!(recent.len(), 1);
    assert!(recent[0].success);
    assert_eq!(recent[0].duration_ms, 42);
}

#[tokio::test]
async fn installations_roundtrip() {
    let pool = fresh_pool().await;
    let repo = InstallationsRepository::new(&pool);

    let now = Utc::now();
    let inst = ToolInstallationRow {
        id: Uuid::new_v4(),
        tool_name: "claude-code".to_string(),
        project_id: None,
        adapter_version: "0.2.0".to_string(),
        installed_at: now,
        last_verified_at: Some(now),
        status: "active".to_string(),
        metadata: serde_json::json!({"path": "/usr/local/bin/claude"}),
    };
    repo.upsert_installation(&inst).await.unwrap();

    let found = repo
        .find_installation("claude-code", None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.id, inst.id);
    assert_eq!(found.adapter_version, "0.2.0");

    let comp = InstalledComponentRow {
        id: Uuid::new_v4(),
        installation_id: inst.id,
        component_type: "skill".to_string(),
        component_slug: "code-review".to_string(),
        installed_version: "1.2.3".to_string(),
        installed_path: "/skills/code-review.md".to_string(),
        checksum: "deadbeef".to_string(),
        status: "current".to_string(),
        last_checked_at: Some(now),
    };
    repo.upsert_component(&comp).await.unwrap();

    let comps = repo.list_components(inst.id).await.unwrap();
    assert_eq!(comps.len(), 1);
    assert_eq!(comps[0].component_slug, "code-review");
}

#[tokio::test]
async fn read_state_roundtrip() {
    let pool = fresh_pool().await;
    let repo = ReadStateRepository::new(&pool);

    repo.mark_read("agent", "claude-code", None, None)
        .await
        .unwrap();

    let state = repo
        .get("agent", "claude-code", None)
        .await
        .unwrap()
        .expect("inserted state should be retrievable");
    assert_eq!(state.actor_type, "agent");
    assert_eq!(state.actor_id, "claude-code");
    assert!(state.last_seen_event_id.is_none());

    // Second call with a fresh last_seen_event_id should upsert, not insert.
    let event_id = Uuid::new_v4();
    repo.mark_read("agent", "claude-code", None, Some(event_id))
        .await
        .unwrap();
    let state2 = repo
        .get("agent", "claude-code", None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state2.last_seen_event_id, Some(event_id));
}

#[tokio::test]
async fn tasks_roundtrip() {
    let pool = fresh_pool().await;
    let repo = TasksRepository::new(&pool);

    let now = Utc::now();
    let task = TaskRow {
        id: Uuid::new_v4(),
        project_id: None,
        title: "Ship v0.2".to_string(),
        description: Some("SQLite rewrite".to_string()),
        status: "open".to_string(),
        priority: "high".to_string(),
        assignee: Some("pavle".to_string()),
        due_at: Some(now + chrono::Duration::days(1)),
        metadata: serde_json::json!({"tags": ["foundation"]}),
        created_at: now,
        updated_at: now,
    };
    repo.upsert_task(&task).await.unwrap();

    let active = repo.list_active(None, 10).await.unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].title, "Ship v0.2");

    let goal = GoalRow {
        id: Uuid::new_v4(),
        project_id: None,
        title: "2 paying SS clients".to_string(),
        description: None,
        target_date: Some(NaiveDate::from_ymd_opt(2026, 6, 10).unwrap()),
        status: "active".to_string(),
        metadata: serde_json::json!({}),
        created_at: now,
        updated_at: now,
    };
    repo.upsert_goal(&goal).await.unwrap();
    let goals = repo.list_goals(None).await.unwrap();
    assert_eq!(goals.len(), 1);
    assert_eq!(goals[0].target_date, goal.target_date);

    let decision = DecisionRow {
        id: Uuid::new_v4(),
        project_id: None,
        title: "SQLite over Postgres".to_string(),
        rationale: Some("Local-first, zero Docker".to_string()),
        decided_at: now,
        decided_by: Some("pavle".to_string()),
        metadata: serde_json::json!({"source": "djordje"}),
    };
    repo.save_decision(&decision).await.unwrap();

    let review = ReviewItemRow {
        id: Uuid::new_v4(),
        project_id: None,
        kind: "drift".to_string(),
        title: "Skill drift detected".to_string(),
        body: Some("checksum mismatch".to_string()),
        status: "open".to_string(),
        created_at: now,
        metadata: serde_json::json!({}),
    };
    repo.create_review_item(&review).await.unwrap();
}

#[tokio::test]
async fn migrations_are_idempotent() {
    // Running migrations twice on the same pool should be a no-op.
    let pool = fresh_pool().await;
    run_migrations(&pool)
        .await
        .expect("second migrate run should be a no-op");
}

#[tokio::test]
async fn file_backed_pool_creates_db_with_parent_dir() {
    // Real-world usage targets a relative path inside `.altevra/`. We make
    // sure `create_pool` handles parent directory creation transparently.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("nested/dir/altevra.db");
    let path_str = path.to_string_lossy().into_owned();

    let pool = create_pool(&path_str)
        .await
        .expect("file-backed pool should succeed");
    run_migrations(&pool).await.unwrap();

    let events = EventsRepository::new(&pool);
    let evt = Event::new(
        EventType::SessionStarted,
        "file-backed sanity",
        "test::file",
        ActorType::System,
    );
    events.insert(&evt).await.unwrap();

    assert!(
        path.exists(),
        "sqlite file should have been created on disk"
    );
    drop(pool);
}

/// T1.13: ALL durable writers (not just learnings) route through the single
/// index-maintenance point — a written decision AND a written wiki page enter
/// `object_index` (packet candidate) AND `object_fts` (full-text searchable).
#[tokio::test]
async fn t1_13_all_writers_indexed() {
    let pool = fresh_pool().await;
    let now = Utc::now();

    // --- decision: save_decision_indexed carries a scanned verdict → indexed ---
    let decision = DecisionRow {
        id: Uuid::new_v4(),
        project_id: None,
        title: "Adopt FTS5 for keyless retrieval".to_string(),
        rationale: Some("BM25 over object_fts; embeddings stay optional (R12)".to_string()),
        decided_at: now,
        decided_by: Some("pavle".to_string()),
        metadata: serde_json::json!({}),
    };
    let dec_idx = DecisionIndexEnvelope {
        status: "active".into(),
        sensitivity: "internal".into(),
        domain: "project".into(),
        scope: None,
        categories: "[\"retrieval\"]".into(),
        tags: "[\"fts\",\"retrieval\"]".into(),
        redaction_status: "clean".into(),
    };
    let tasks = TasksRepository::new(&pool);
    tasks
        .save_decision_indexed(&decision, &dec_idx)
        .await
        .unwrap();

    // --- wiki page: upsert_indexed with a scanned verdict → indexed ---
    let wiki = WikiPagesRepository::new(&pool);
    let wiki_id = wiki
        .upsert_indexed(
            "context-packets",
            "context-packets",
            "wiki/concepts/context-packets.md",
            "living",
            "high",
            "internal",
            2,
            Some(now),
            Some("Context Packets"),
            "sha-ctx",
            "project",
            "[\"architecture\"]",
            "[\"packet\",\"retrieval\"]",
            "Context packets turn broad capture into precise gated context.",
            "clean",
        )
        .await
        .unwrap();

    // structured index: BOTH objects are packet candidates.
    let idx = ObjectIndexRepository::new(&pool);
    let candidates = idx.candidates(Some("project")).await.unwrap();
    assert!(
        candidates
            .iter()
            .any(|c| c.object_type == "decision" && c.id == decision.id.to_string()),
        "decision must be a packet candidate in object_index"
    );
    assert!(
        candidates
            .iter()
            .any(|c| c.object_type == "wiki" && c.id == wiki_id.to_string()),
        "wiki page must be a packet candidate in object_index"
    );

    // full-text: BOTH objects are searchable via object_fts (bm25).
    let fts = FtsRepository::new(&pool);
    let dec_hits = fts.search("FTS5 keyless retrieval", 10).await.unwrap();
    assert!(
        dec_hits
            .iter()
            .any(|h| h.object_type == "decision" && h.object_id == decision.id.to_string()),
        "decision must be full-text searchable"
    );
    let wiki_hits = fts.search("context packets capture", 10).await.unwrap();
    assert!(
        wiki_hits
            .iter()
            .any(|h| h.object_type == "wiki" && h.object_id == wiki_id.to_string()),
        "wiki page must be full-text searchable"
    );
}

/// R5: the exposure-decision writer inserts a content-free aggregate row — no
/// raw object id/title/body of any candidate is persisted (§2.13 no existence
/// leak); only counts + the request ceiling/scope are stored.
#[tokio::test]
async fn exposure_decision_written() {
    let pool = fresh_pool().await;
    let repo = ExposureDecisionsRepository::new(&pool);
    let audit = ExposureAudit {
        packet_id: None,
        sensitivity_ceiling: "internal".into(),
        domain_scope: vec!["business".into(), "project".into()],
        included_count: 4,
        excluded_count: 2,
        excluded_by_reason: vec![("over_sensitivity_ceiling".into(), 2)],
        redaction_counts: vec![("clean".into(), 4)],
        truncated: false,
    };
    let id = repo.insert(&audit).await.unwrap();

    // exactly one row exists, and the content-free ref columns carry no {type,id}.
    let (cnt,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM exposure_decisions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(cnt, 1, "one audit row per compile");

    let (included_refs, excluded_refs, request): (String, String, String) = sqlx::query_as(
        "SELECT included_refs, excluded_refs, request FROM exposure_decisions WHERE id = ?",
    )
    .bind(id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    // aggregate counts present...
    assert!(included_refs.contains("\"count\":4"));
    assert!(excluded_refs.contains("\"count\":2"));
    // ...but NO per-object id/title/body — never leak existence of denied items.
    assert!(!included_refs.contains("\"id\""));
    assert!(!excluded_refs.contains("\"id\""));
    assert!(!included_refs.contains("\"title\""));
    assert!(!excluded_refs.contains("\"title\""));
    // the request echo is the content-free envelope (ceiling + scope only).
    assert!(request.contains("internal"));
    assert!(request.contains("project"));
}


/// B3 substrate: `decisions_due_for_review` returns only `active` decisions whose
/// `review_after` is in the past — the "decision still valid?" daily-briefing seed.
#[tokio::test]
async fn decisions_due_for_review_filters_by_review_after_and_status() {
    let pool = fresh_pool().await;
    let repo = TasksRepository::new(&pool);

    // Three decisions: one due (review_after in the past), one not yet due, one
    // due but already superseded (status != active → excluded).
    let due = DecisionRow {
        id: Uuid::new_v4(),
        project_id: None,
        title: "Stop building, start selling".into(),
        rationale: Some("Đorđe directive".into()),
        decided_at: "2026-04-10T00:00:00Z".parse().unwrap(),
        decided_by: Some("djordje".into()),
        metadata: serde_json::json!({}),
    };
    let future = DecisionRow {
        id: Uuid::new_v4(),
        project_id: None,
        title: "Adopt SQLite local-first".into(),
        rationale: None,
        decided_at: "2026-05-01T00:00:00Z".parse().unwrap(),
        decided_by: None,
        metadata: serde_json::json!({}),
    };
    let superseded = DecisionRow {
        id: Uuid::new_v4(),
        project_id: None,
        title: "Old GTM plan".into(),
        rationale: None,
        decided_at: "2026-03-01T00:00:00Z".parse().unwrap(),
        decided_by: None,
        metadata: serde_json::json!({}),
    };
    for d in [&due, &future, &superseded] {
        repo.save_decision(d).await.unwrap();
    }

    // Set review_after / status directly (save_decision defaults status='active'
    // and leaves review_after NULL).
    sqlx::query("UPDATE decisions SET review_after = ? WHERE id = ?")
        .bind("2026-05-01T00:00:00.000Z")
        .bind(due.id.to_string())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE decisions SET review_after = ? WHERE id = ?")
        .bind("2026-12-01T00:00:00.000Z")
        .bind(future.id.to_string())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE decisions SET review_after = ?, status = 'superseded' WHERE id = ?")
        .bind("2026-05-01T00:00:00.000Z")
        .bind(superseded.id.to_string())
        .execute(&pool)
        .await
        .unwrap();

    let now = "2026-06-03T00:00:00Z".parse().unwrap();
    let dues = repo.decisions_due_for_review(now, 10).await.unwrap();
    assert_eq!(dues.len(), 1, "only the active, past-review decision");
    assert_eq!(dues[0].id, due.id.to_string());
    assert_eq!(dues[0].title, "Stop building, start selling");
}

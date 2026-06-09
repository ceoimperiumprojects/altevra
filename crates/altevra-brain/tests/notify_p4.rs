//! P4 gate suite (PLAN-ALIVE §P4) — hermetic tests for the proactive
//! briefing + notification framework + observer backfill.
//!
//! Pinned invariants:
//!   1. brief-from-fixture renders valid `daily_briefing_v1` markdown
//!      (frontmatter + every section header verbatim);
//!   2. a relationship item is dropped from the Obsidian path BY POLICY
//!      (`dp_relationship.obsidian_mirror = 'never'`) and audited; the gated
//!      render carries only a count + `altevra brief --private` pointer;
//!   3. FAIL-CLOSED: a policy lookup ERROR or a missing policy row drops the
//!      item from the Obsidian path + writes an `audit_log` row;
//!   4. an unflagged rule (`user_visible_only` default TRUE) NEVER reaches
//!      the agent channel;
//!   5. O_EXCL claim dedup suppresses the second fire in a cadence window;
//!   6. backfill run twice → ZERO duplicate events;
//!   7. backfill events are METADATA-ONLY — no turn body text anywhere, and
//!      timestamps are HISTORICAL (invisible to rolling windows);
//!   8. the relevance gate drops an off-interest item from briefing research
//!      selection.
//!
//! Per-test TempDir DBs + temp vaults — never the real ~/.altevra or a real
//! Obsidian vault. (interests.yaml template-creation + research-feed gating
//! are covered by `altevra-research/src/interests.rs` unit tests.)

use altevra_brain::notify::{self, NotifyItem, RULE_RELATIONSHIP_CADENCE, RULE_RESUME_BRIEF};
use altevra_brain::run_observer_backfill;
use altevra_db::{create_pool, run_migrations};
use altevra_research::{Interest, RelevanceGate};
use chrono::Utc;
use sqlx::SqlitePool;
use uuid::Uuid;

/// File-backed TempDir pool (the TempDir must outlive the pool).
async fn mk_pool(tmp: &tempfile::TempDir) -> SqlitePool {
    let db = tmp.path().join("test.db");
    let pool = create_pool(&db.to_string_lossy()).await.unwrap();
    run_migrations(&pool).await.unwrap();
    pool
}

fn cfg(tmp: &tempfile::TempDir, claim: bool) -> notify::DeliveryConfig {
    notify::DeliveryConfig {
        claims_dir: tmp.path().join("claims"),
        claim,
    }
}

async fn audit_rows(pool: &SqlitePool, action: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM audit_log WHERE action = ?")
        .bind(action)
        .fetch_one(pool)
        .await
        .unwrap()
}

// ---- 4. agent channel ------------------------------------------------------

#[tokio::test]
async fn unflagged_rule_never_reaches_agent_channel() {
    let tmp = tempfile::TempDir::new().unwrap();
    let pool = mk_pool(&tmp).await;

    let default_item = NotifyItem::new(RULE_RESUME_BRIEF, "project", "default", "", "k1");
    let opted_out =
        NotifyItem::new(RULE_RESUME_BRIEF, "project", "opted", "", "k2").allow_agent_channel();

    let out = notify::deliver(
        &pool,
        &cfg(&tmp, false),
        vec![default_item, opted_out],
        Utc::now(),
    )
    .await
    .unwrap();

    assert_eq!(out.user_visible.len(), 2, "both reach the user channel");
    assert_eq!(
        out.agent_visible.len(),
        1,
        "ONLY the explicit opt-out reaches the agent channel"
    );
    assert_eq!(out.agent_visible[0].title, "opted");
}

// ---- 2. relationship policy gate --------------------------------------------

#[tokio::test]
async fn relationship_item_blocked_from_obsidian_path_by_policy() {
    let tmp = tempfile::TempDir::new().unwrap();
    let pool = mk_pool(&tmp).await;

    let item = NotifyItem::new(
        RULE_RELATIONSHIP_CADENCE,
        "relationship",
        "haven't talked to Srdjan in 6 weeks",
        "",
        "contact-gap:srdjan",
    );
    let out = notify::deliver(&pool, &cfg(&tmp, false), vec![item], Utc::now())
        .await
        .unwrap();

    assert!(out.obsidian.is_empty(), "never mirrored to the vault path");
    assert_eq!(out.obsidian_blocked.len(), 1, "blocked but kept user-visible");
    assert_eq!(out.user_visible.len(), 1, "still on the local-only channel");
    assert!(
        audit_rows(&pool, "notify_policy_denied").await >= 1,
        "policy denial must be audited"
    );

    // Gated render: COUNT + CLI pointer only — never the person's name.
    let gate = RelevanceGate::default();
    let data = notify::build_brief_data(&pool, &out, &gate, Utc::now()).await;
    let gated = notify::render_brief(&data, false);
    assert!(
        !gated.contains("Srdjan"),
        "a name must never land in the vault-bound render:\n{gated}"
    );
    assert!(gated.contains("1 private signal(s) withheld by domain policy"));
    assert!(gated.contains("altevra brief --private"), "CLI pointer present");

    // Private render (terminal-only) carries the full line.
    let private = notify::render_brief(&data, true);
    assert!(private.contains("Srdjan"), "private render shows the signal");
}

// ---- 3. fail-closed ----------------------------------------------------------

#[tokio::test]
async fn fail_closed_missing_policy_row_drops_item_and_audits() {
    let tmp = tempfile::TempDir::new().unwrap();
    let pool = mk_pool(&tmp).await;

    let item = NotifyItem::new(RULE_RESUME_BRIEF, "no_such_domain", "mystery", "", "k");
    let out = notify::deliver(&pool, &cfg(&tmp, false), vec![item], Utc::now())
        .await
        .unwrap();

    assert!(out.obsidian.is_empty(), "missing policy ⇒ dropped (fail-closed)");
    assert!(out.obsidian_blocked.is_empty());
    assert_eq!(out.dropped.len(), 1);
    assert!(out.dropped[0].1.contains("no domain policy"));
    assert!(
        audit_rows(&pool, "notify_policy_drop").await >= 1,
        "fail-closed drop must write an audit_log row"
    );
}

#[tokio::test]
async fn fail_closed_policy_lookup_error_drops_item_and_audits() {
    let tmp = tempfile::TempDir::new().unwrap();
    let pool = mk_pool(&tmp).await;

    // Force a real lookup ERROR (not a miss): the policy table is gone.
    sqlx::query("ALTER TABLE domain_policies RENAME TO domain_policies_gone")
        .execute(&pool)
        .await
        .unwrap();

    let item = NotifyItem::new(RULE_RESUME_BRIEF, "business", "anything", "", "k");
    let out = notify::deliver(&pool, &cfg(&tmp, false), vec![item], Utc::now())
        .await
        .unwrap();

    assert!(out.obsidian.is_empty(), "lookup error ⇒ dropped (fail-closed)");
    assert_eq!(out.dropped.len(), 1);
    assert!(out.dropped[0].1.contains("policy lookup error"));
    assert!(
        audit_rows(&pool, "notify_policy_drop").await >= 1,
        "lookup-error drop must write an audit_log row"
    );
    // The item stays user-visible — fail-closed applies to the SYNCABLE path.
    assert_eq!(out.user_visible.len(), 1);
}

// ---- 5. O_EXCL dedup ---------------------------------------------------------

#[tokio::test]
async fn oexcl_claim_dedup_suppresses_second_fire() {
    let tmp = tempfile::TempDir::new().unwrap();
    let pool = mk_pool(&tmp).await;
    let now = Utc::now();
    let mk = || NotifyItem::new(RULE_RESUME_BRIEF, "project", "resume", "", "same-fact");

    let first = notify::deliver(&pool, &cfg(&tmp, true), vec![mk()], now)
        .await
        .unwrap();
    assert_eq!(first.user_visible.len(), 1, "first fire delivers");
    assert!(first.suppressed.is_empty());

    let second = notify::deliver(&pool, &cfg(&tmp, true), vec![mk()], now)
        .await
        .unwrap();
    assert!(
        second.user_visible.is_empty(),
        "second fire in the same cadence window is suppressed"
    );
    assert_eq!(second.suppressed.len(), 1);
    assert_eq!(second.suppressed[0].1, "same-fact");

    // A DIFFERENT fact is not suppressed by the first claim.
    let other = NotifyItem::new(RULE_RESUME_BRIEF, "project", "resume", "", "other-fact");
    let third = notify::deliver(&pool, &cfg(&tmp, true), vec![other], now)
        .await
        .unwrap();
    assert_eq!(third.user_visible.len(), 1);
}

// ---- 1. brief from fixture ----------------------------------------------------

#[tokio::test]
async fn brief_from_fixture_matches_daily_briefing_v1() {
    use altevra_db::{DecisionRow, NewProposal, ProposalsRepository, SessionsRepository, TasksRepository};

    let tmp = tempfile::TempDir::new().unwrap();
    let pool = mk_pool(&tmp).await;
    let vault = tmp.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();
    let now = Utc::now();

    // Session with a summary, recently ended → resume-brief source.
    let sessions = SessionsRepository::new(&pool);
    let sid = Uuid::new_v4();
    sessions
        .start_session(&altevra_db::SessionRow {
            id: sid,
            tool: "claude-code".into(),
            project_id: None,
            project_name: Some("altevra".into()),
            started_at: now - chrono::Duration::hours(3),
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
    sessions
        .end_session(sid, Some("wired the P4 notify framework"))
        .await
        .unwrap();

    // Decision past its review_after → decision-staleness source.
    let tasks = TasksRepository::new(&pool);
    let did = Uuid::new_v4();
    tasks
        .save_decision(&DecisionRow {
            id: did,
            project_id: None,
            title: "one canonical DB".into(),
            rationale: None,
            decided_at: now - chrono::Duration::days(120),
            decided_by: Some("pavle".into()),
            metadata: serde_json::json!({}),
        })
        .await
        .unwrap();
    sqlx::query("UPDATE decisions SET review_after = ? WHERE id = ?")
        .bind((now - chrono::Duration::days(1)).to_rfc3339())
        .bind(did.to_string())
        .execute(&pool)
        .await
        .unwrap();

    // Open proposal → open-proposals source.
    ProposalsRepository::new(&pool)
        .insert(&NewProposal {
            kind: "improvement".into(),
            title: "tighten redaction".into(),
            body: "".into(),
            source_mode: None,
            dedup_hash: "p4-fixture".into(),
            evidence_refs: vec![],
            touches_sensitive: false,
            touches_constitutional: false,
        })
        .await
        .unwrap();

    // Scored research item → Useful Research section.
    sqlx::query(
        "INSERT INTO research_items (id, feed_id, guid, link, title, summary, published_at, relevance_score) \
         VALUES (?, 'f', 'g', 'https://example.com/rust', 'Rust embeddings deep dive', 'sqlite vectors', ?, 0.9)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(now.to_rfc3339())
    .execute(&pool)
    .await
    .unwrap();

    let items = notify::sources::collect_all(&pool, &vault, now).await;
    assert!(!items.is_empty(), "fixture must produce candidate items");
    let delivery = notify::deliver(&pool, &cfg(&tmp, false), items, now)
        .await
        .unwrap();
    let gate = RelevanceGate::default();
    let data = notify::build_brief_data(&pool, &delivery, &gate, now).await;
    let md = notify::render_brief(&data, false);

    // daily_briefing_v1: frontmatter keys + verbatim section headers.
    assert!(md.starts_with("---\nkind: altevra-daily-brief\n"), "{md}");
    for key in ["generated_by:", "date:", "mode: daily_briefing", "schema_version: 1", "confidence:"] {
        assert!(md.contains(key), "frontmatter missing {key}:\n{md}");
    }
    assert!(md.contains(&format!("# Daily Brief — {}", now.format("%Y-%m-%d"))));
    for header in [
        "## What Changed",
        "## What Matters",
        "## Decisions",
        "## Tasks Needing Attention",
        "## Useful Research",
        "## Risks",
        "## Suggested Focus",
    ] {
        assert!(md.contains(&format!("\n{header}\n")), "missing header {header}:\n{md}");
    }
    // Sections carry the fixture signals.
    assert!(md.contains("wired the P4 notify framework"), "resume brief:\n{md}");
    assert!(md.contains("one canonical DB"), "stale decision:\n{md}");
    assert!(md.contains("proposal(s) awaiting review"), "open proposals:\n{md}");
    assert!(md.contains("Rust embeddings deep dive"), "research line:\n{md}");
    // No blocked items in this fixture → Personal Signals omitted entirely.
    assert!(!md.contains("## Personal Signals"), "{md}");

    // write_vault_brief: lands in <vault>/Daily/<date>-altevra-brief.md, idempotent.
    let p = notify::write_vault_brief(&pool, &vault, &tmp.path().join("claims"), false, &gate, now)
        .await
        .unwrap()
        .expect("first write returns the path");
    assert_eq!(
        p,
        vault.join("Daily").join(format!("{}-altevra-brief.md", now.format("%Y-%m-%d")))
    );
    assert!(p.exists());
    let again = notify::write_vault_brief(&pool, &vault, &tmp.path().join("claims"), false, &gate, now)
        .await
        .unwrap();
    assert!(again.is_none(), "second write same day is a no-op");
}

// ---- 8. relevance gate in briefing selection -----------------------------------

#[tokio::test]
async fn relevance_gate_drops_off_interest_item_from_brief_research() {
    let tmp = tempfile::TempDir::new().unwrap();
    let pool = mk_pool(&tmp).await;
    let now = Utc::now();

    for (title, summary) in [
        ("Rust 1.80 SQLite embeddings", "vectors in sqlite"),
        ("Top 10 Minecraft modpacks of 2026", "blocky fun"),
    ] {
        sqlx::query(
            "INSERT INTO research_items (id, feed_id, guid, link, title, summary, published_at, relevance_score) \
             VALUES (?, 'f', ?, 'https://example.com', ?, ?, ?, 0.9)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(title)
        .bind(title)
        .bind(summary)
        .bind(now.to_rfc3339())
        .execute(&pool)
        .await
        .unwrap();
    }

    let gate = RelevanceGate::from_interests(vec![Interest {
        name: "rust".into(),
        keywords: vec!["rust".into(), "sqlite".into()],
        domains: vec![],
        enabled: true,
    }]);
    let delivery = notify::Delivery::default();
    let data = notify::build_brief_data(&pool, &delivery, &gate, now).await;

    assert!(
        data.research.iter().any(|l| l.contains("Rust 1.80")),
        "on-interest item kept: {:?}",
        data.research
    );
    assert!(
        !data.research.iter().any(|l| l.contains("Minecraft")),
        "off-interest item dropped by the relevance gate: {:?}",
        data.research
    );

    // Inactive gate (no stated interests) → legacy behavior, both pass.
    let inactive = RelevanceGate::default();
    let data2 = notify::build_brief_data(&pool, &delivery, &inactive, now).await;
    assert_eq!(data2.research.len(), 2);
}

// ---- 6 + 7. observer backfill ---------------------------------------------------

async fn seed_turns(pool: &SqlitePool, body: &str, old: chrono::DateTime<Utc>) -> Uuid {
    use altevra_db::{SessionRow, SessionsRepository, TurnRow};
    let sessions = SessionsRepository::new(pool);
    let sid = Uuid::new_v4();
    sessions
        .start_session(&SessionRow {
            id: sid,
            tool: "claude-code".into(),
            project_id: None,
            project_name: None,
            started_at: old,
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
    for (idx, (role, tool_name)) in [
        ("user", None),
        ("assistant", None),
        ("tool_call", Some("Bash")),
    ]
    .iter()
    .enumerate()
    {
        sessions
            .record_turn(&TurnRow {
                id: Uuid::new_v4(),
                session_id: sid,
                turn_idx: idx as i64,
                role: role.to_string(),
                content: format!("{body} #{idx}"),
                tool_calls: None,
                tool_name: tool_name.map(String::from),
                model: None,
                tokens_in: None,
                tokens_out: None,
                latency_ms: None,
                file_changes: None,
                redacted_count: 0,
                source_tool: Some("claude-code".into()),
                sensitivity: "internal".into(),
                redaction_status: "clean".into(),
                created_at: old + chrono::Duration::minutes(idx as i64),
                working_dir: None,
            })
            .await
            .unwrap();
    }
    sid
}

#[tokio::test]
async fn backfill_twice_produces_zero_duplicate_events() {
    let tmp = tempfile::TempDir::new().unwrap();
    let pool = mk_pool(&tmp).await;
    let old = Utc::now() - chrono::Duration::days(100);
    seed_turns(&pool, "ordinary work content", old).await;

    let first = run_observer_backfill(&pool).await.unwrap();
    assert_eq!(first.turns_seen, 3);
    assert_eq!(first.events_inserted, 3);
    let count1: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE source = 'backfill'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count1, 3);

    let second = run_observer_backfill(&pool).await.unwrap();
    assert_eq!(
        second.events_inserted, 0,
        "re-run inserts ZERO duplicates (deterministic ids + OR IGNORE + watermark)"
    );
    let count2: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE source = 'backfill'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count2, count1, "event count unchanged after second run");

    // Watermark row recorded both runs.
    let runs: i64 = sqlx::query_scalar("SELECT runs FROM observer_backfill_state WHERE id = 'singleton'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(runs, 2);

    // New turns AFTER the watermark are picked up incrementally on a later run.
    seed_turns(&pool, "later corpus", Utc::now() - chrono::Duration::days(50)).await;
    let third = run_observer_backfill(&pool).await.unwrap();
    assert_eq!(third.events_inserted, 3, "incremental sweep picks up only new turns");
    let count3: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE source = 'backfill'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count3, 6);
}

#[tokio::test]
async fn backfill_events_are_metadata_only_with_historical_timestamps() {
    let tmp = tempfile::TempDir::new().unwrap();
    let pool = mk_pool(&tmp).await;
    const BODY: &str = "SUPER-SECRET-TURN-BODY do not leak";
    seed_turns(&pool, BODY, Utc::now() - chrono::Duration::days(100)).await;

    let report = run_observer_backfill(&pool).await.unwrap();
    assert_eq!(report.events_inserted, 3);

    // NEVER turn body content — not in title, summary, or payload.
    let rows: Vec<(String, Option<String>, String)> = sqlx::query_as(
        "SELECT title, summary, payload FROM events WHERE source = 'backfill'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 3);
    for (title, summary, payload) in &rows {
        for field in [title.as_str(), summary.as_deref().unwrap_or(""), payload.as_str()] {
            assert!(
                !field.contains("SUPER-SECRET"),
                "turn body leaked into a backfill event: {field}"
            );
        }
    }
    // Metadata IS present: tool name + role + refs + counts.
    let payloads = rows.iter().map(|r| r.2.as_str()).collect::<Vec<_>>().join("\n");
    assert!(payloads.contains("\"tool_name\":\"Bash\""), "{payloads}");
    assert!(payloads.contains("\"content_len\""), "{payloads}");
    assert!(payloads.contains("\"source_turn_id\""), "{payloads}");

    // HISTORICAL timestamps: invisible to a 7-day rolling window, visible to
    // the explicit one-shot epoch scan.
    let repo = altevra_db::EventsRepository::new(&pool);
    let rolling = repo
        .list_since(Utc::now() - chrono::Duration::days(7), None, 100)
        .await
        .unwrap();
    assert!(
        rolling.iter().all(|e| e.source != "backfill"),
        "backfilled events must NOT flood the rolling window"
    );
    let cold_start = repo
        .list_since(report.earliest_event_at.unwrap() - chrono::Duration::seconds(1), None, 100)
        .await
        .unwrap();
    assert_eq!(
        cold_start.iter().filter(|e| e.source == "backfill").count(),
        3,
        "explicit since-epoch scan surfaces the cold-start events"
    );
    assert!(report.scan_since_hint().unwrap().starts_with('@'));
}

//! P5 gate (PLAN-ALIVE §P5.3) — userVisibleOnly enforcement for personal-
//! brain notifications.
//!
//! Personal/relationship/health note content surfaced through the P4 notify
//! layer MUST carry `user_visible_only = true`. That is the DEFAULT
//! (`NotifyItem::new` — opt-out is explicit), and the delivery layer never
//! routes a default item to the agent-injected channel. These tests pin
//! exactly that for the high-water personal-brain domains, plus the policy
//! posture: relationship/health items are also blocked from the syncable
//! Obsidian path (`obsidian_mirror = 'never'`).
//!
//! Hermetic: per-test TempDir DBs, never the real ~/.altevra.

use altevra_brain::notify::{self, NotifyItem, RULE_RELATIONSHIP_CADENCE};
use altevra_db::{create_pool, run_migrations};
use chrono::Utc;
use sqlx::SqlitePool;

async fn mk_pool(tmp: &tempfile::TempDir) -> SqlitePool {
    let db = tmp.path().join("test.db");
    let pool = create_pool(&db.to_string_lossy()).await.unwrap();
    run_migrations(&pool).await.unwrap();
    pool
}

fn cfg(tmp: &tempfile::TempDir) -> notify::DeliveryConfig {
    notify::DeliveryConfig {
        claims_dir: tmp.path().join("claims"),
        claim: false,
    }
}

#[tokio::test]
async fn personal_domain_note_item_never_reaches_agent_channel() {
    // §P5.3: a personal-domain notification built the DEFAULT way (no
    // explicit opt-out) must never land in agent-injected context.
    let tmp = tempfile::TempDir::new().unwrap();
    let pool = mk_pool(&tmp).await;

    let items = vec![
        NotifyItem::new(RULE_RELATIONSHIP_CADENCE, "personal", "mood dip noticed", "", "p1"),
        NotifyItem::new(RULE_RELATIONSHIP_CADENCE, "relationship", "reach out to Srđan", "", "p2"),
        NotifyItem::new(RULE_RELATIONSHIP_CADENCE, "health", "sleep pattern broken", "", "p3"),
    ];
    for it in &items {
        assert!(
            it.user_visible_only,
            "{}-domain item must default to user-visible-only",
            it.domain_key
        );
    }

    let out = notify::deliver(&pool, &cfg(&tmp), items, Utc::now()).await.unwrap();

    assert_eq!(out.user_visible.len(), 3, "all reach the user channel");
    assert!(
        out.agent_visible.is_empty(),
        "a personal/relationship/health item must NEVER reach the agent channel: {:?}",
        out.agent_visible.iter().map(|i| &i.domain_key).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn relationship_and_health_items_also_blocked_from_obsidian_by_policy() {
    // Defense in depth: even on the user-facing side, dp_relationship /
    // dp_health seed `obsidian_mirror = 'never'` — the syncable vault never
    // sees the content (P4 fail-closed gate; pinned here for the P5 note
    // domains so a policy regression is caught at this layer too).
    let tmp = tempfile::TempDir::new().unwrap();
    let pool = mk_pool(&tmp).await;

    let out = notify::deliver(
        &pool,
        &cfg(&tmp),
        vec![
            NotifyItem::new(RULE_RELATIONSHIP_CADENCE, "relationship", "Elena anniversary", "", "r1"),
            NotifyItem::new(RULE_RELATIONSHIP_CADENCE, "health", "knee pain trend", "", "h1"),
        ],
        Utc::now(),
    )
    .await
    .unwrap();

    assert!(out.obsidian.is_empty(), "relationship/health must not reach Obsidian");
    assert_eq!(out.obsidian_blocked.len(), 2, "blocked by obsidian_mirror=never");
    assert!(out.agent_visible.is_empty());
}

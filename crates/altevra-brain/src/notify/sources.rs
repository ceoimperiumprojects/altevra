//! Notification SOURCES (P4) — each produces candidate [`NotifyItem`]s from
//! the canonical stores. All are **high-precision-or-silent**: an empty Vec /
//! `None` over a weak signal, never a filler bullet. All are fail-soft
//! (DB errors → empty), so a broken source never aborts the briefing.

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use std::path::Path;

use super::types::{
    NotifyItem, RULE_DECISION_STALENESS, RULE_OPEN_PROPOSALS, RULE_RELATIONSHIP_CADENCE,
    RULE_RESUME_BRIEF,
};

/// A person who's gone quiet: no mention for N weeks (CLAUDE.md §3.6).
const LAST_CONTACT_STALE_WEEKS: i64 = 2;
/// Resume brief only references sessions newer than this.
const RESUME_WINDOW_DAYS: i64 = 7;

/// "You decided X — still applies?" — decisions past their `review_after`.
/// Domain: business (the decisions store carries no per-row domain; the
/// most-restrictive plausible default for decision titles).
pub async fn decision_staleness(pool: &SqlitePool, now: DateTime<Utc>) -> Vec<NotifyItem> {
    use altevra_db::TasksRepository;

    let due = TasksRepository::new(pool)
        .decisions_due_for_review(now, 20)
        .await
        .unwrap_or_default();
    due.iter()
        .map(|d| {
            NotifyItem::new(
                RULE_DECISION_STALENESS,
                "business",
                format!(
                    "decision '{}' from {} — still applies?",
                    d.title,
                    d.decided_at.format("%Y-%m-%d")
                ),
                String::new(),
                format!("decision:{}", d.id),
            )
        })
        .collect()
}

/// "Haven't talked to <Person> in N weeks" — mention-graph contact gaps.
/// Domain: relationship — `dp_relationship` is seeded `obsidian_mirror =
/// 'never'`, so the delivery layer keeps these OUT of the Obsidian path;
/// they surface as a count + CLI pointer there and in full via
/// `altevra brief --private` (terminal only). `user_visible_only` stays at
/// its TRUE default — relationship items must never reach agent context.
pub async fn relationship_cadence(
    pool: &SqlitePool,
    vault: &Path,
    now: DateTime<Utc>,
) -> Vec<NotifyItem> {
    use altevra_core::{last_contact, EntityKind};
    use altevra_db::MentionsRepository;

    let dict = altevra_vault::entity_dict::build_dictionary_for_vault(vault);
    let dated = MentionsRepository::new(pool)
        .dated_mentions()
        .await
        .unwrap_or_default();
    let today = now.date_naive();
    let mut items: Vec<NotifyItem> = Vec::new();
    for person in dict.all().filter(|e| e.kind == EntityKind::Person) {
        let Some(last) = last_contact(&person.id, &dated) else {
            continue; // never mentioned → nothing to nag about
        };
        let weeks = (today - last).num_weeks();
        if weeks >= LAST_CONTACT_STALE_WEEKS {
            items.push(NotifyItem::new(
                RULE_RELATIONSHIP_CADENCE,
                "relationship",
                format!(
                    "haven't talked to {} in {} weeks (last: {})",
                    person.name, weeks, last
                ),
                String::new(),
                format!("contact-gap:{}", person.id),
            ));
        }
    }
    items.sort_by(|a, b| a.title.cmp(&b.title));
    items
}

/// "Where you left off" — the most recent ENDED session in the window that
/// actually carries a summary or a project name. Silent otherwise (never a
/// stale pointer, never "nothing pending").
pub async fn resume_brief(pool: &SqlitePool, now: DateTime<Utc>) -> Option<NotifyItem> {
    use altevra_db::SessionsRepository;

    let sessions = SessionsRepository::new(pool)
        .list_sessions(None, None, 10)
        .await
        .ok()?;
    let window_start = now - chrono::Duration::days(RESUME_WINDOW_DAYS);
    let last = sessions
        .iter()
        // Ended (not still live), recent, and with something to say.
        .filter(|s| s.ended_at.is_some_and(|e| e > window_start))
        .filter(|s| s.summary.as_deref().is_some_and(|x| !x.trim().is_empty()) || s.project_name.is_some())
        .max_by_key(|s| s.ended_at)?;

    let project = last
        .project_name
        .clone()
        .unwrap_or_else(|| last.tool.clone());
    let summary = last
        .summary
        .clone()
        .unwrap_or_else(|| format!("{} turns recorded", last.turn_count));
    Some(NotifyItem::new(
        RULE_RESUME_BRIEF,
        "project",
        format!("picking up on {project} — where you left off"),
        summary,
        format!("resume:{}", last.id),
    ))
}

/// "N proposals awaiting your review" — open items in the trust-ladder
/// review queue.
pub async fn open_proposals(pool: &SqlitePool) -> Option<NotifyItem> {
    use altevra_db::ProposalsRepository;

    let proposals = ProposalsRepository::new(pool)
        .list(Some("proposed"), None)
        .await
        .ok()?;
    if proposals.is_empty() {
        return None; // silence over "0 proposals"
    }
    let newest = &proposals[0];
    let titles: Vec<String> = proposals.iter().take(3).map(|p| p.title.clone()).collect();
    Some(NotifyItem::new(
        RULE_OPEN_PROPOSALS,
        "business",
        format!("{} proposal(s) awaiting review", proposals.len()),
        titles.join("; "),
        // Re-fires when the queue head or size changes.
        format!("proposals:{}:{}", proposals.len(), newest.id),
    ))
}

/// All P4 sources in briefing order.
pub async fn collect_all(pool: &SqlitePool, vault: &Path, now: DateTime<Utc>) -> Vec<NotifyItem> {
    let mut items = Vec::new();
    if let Some(i) = resume_brief(pool, now).await {
        items.push(i);
    }
    items.extend(decision_staleness(pool, now).await);
    items.extend(relationship_cadence(pool, vault, now).await);
    if let Some(i) = open_proposals(pool).await {
        items.push(i);
    }
    items
}

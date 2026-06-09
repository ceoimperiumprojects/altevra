//! Notification DELIVERY (P4) — routes claimed items per channel with the
//! two load-bearing gates:
//!
//!   1. **Agent channel gate:** `user_visible_only == true` (the default)
//!      NEVER reaches agent-injected context. Only explicitly opted-out
//!      items do.
//!   2. **Obsidian policy gate, per item, FAIL-CLOSED:** every item bound for
//!      the (syncable) Obsidian vault consults
//!      `domain_policies.obsidian_mirror` for its `domain_key`. A lookup
//!      ERROR or a MISSING policy row drops the item from the Obsidian path
//!      and writes an `audit_log` row. `obsidian_mirror = 'never'` (e.g.
//!      `dp_relationship`) blocks the Obsidian path too (audited) — the item
//!      remains available on the local-only terminal channel
//!      (`altevra brief --private`).
//!
//! Dedup: one notification per item per cadence window via an atomic
//! `O_CREAT|O_EXCL` claim file named `{rule}-{sha256[:12](dedup_key)}-{bucket}`
//! (hashed so a relationship dedup key never leaks a name into a filename).

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::path::PathBuf;

use super::types::{cadence_bucket, NotifyItem};

/// Default claim-file directory: `ALTEVRA_NOTIFY_CLAIMS_DIR` (tests /
/// overrides) or `~/.altevra/state/notify-claims`.
pub fn default_claims_dir() -> PathBuf {
    if let Ok(p) = std::env::var("ALTEVRA_NOTIFY_CLAIMS_DIR") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    altevra_core::home_dir().join(".altevra/state/notify-claims")
}

/// Delivery configuration.
#[derive(Debug, Clone)]
pub struct DeliveryConfig {
    /// Directory holding O_EXCL claim files (one per item per cadence window).
    pub claims_dir: PathBuf,
    /// When false the run is a read-only VIEW (e.g. `altevra brief` in the
    /// terminal): no claims are taken, nothing is suppressed by dedup.
    pub claim: bool,
}

/// Routed output of one delivery pass.
#[derive(Debug, Default)]
pub struct Delivery {
    /// Items for user-facing channels (terminal, gated brief rendering).
    pub user_visible: Vec<NotifyItem>,
    /// Items eligible for agent-injected context — ONLY explicit opt-outs.
    pub agent_visible: Vec<NotifyItem>,
    /// Items the per-item policy gate allows into the Obsidian vault.
    pub obsidian: Vec<NotifyItem>,
    /// Items policy-blocked from Obsidian (`obsidian_mirror = 'never'`) but
    /// still user-visible locally. Rendered in the Obsidian brief only as a
    /// count + CLI pointer.
    pub obsidian_blocked: Vec<NotifyItem>,
    /// `(rule, reason)` — items dropped from the Obsidian path FAIL-CLOSED
    /// (policy lookup error / missing policy row). Audited.
    pub dropped: Vec<(String, String)>,
    /// `(rule, dedup_key)` — suppressed by the cadence-window claim.
    pub suppressed: Vec<(String, String)>,
}

/// Route `items` through dedup + channel gates. Never includes an item in
/// `agent_visible` unless it explicitly opted out of `user_visible_only`.
pub async fn deliver(
    pool: &SqlitePool,
    cfg: &DeliveryConfig,
    items: Vec<NotifyItem>,
    now: DateTime<Utc>,
) -> anyhow::Result<Delivery> {
    use altevra_db::DomainPolicyRepository;

    let mut out = Delivery::default();
    if cfg.claim {
        std::fs::create_dir_all(&cfg.claims_dir)?;
    }
    let policy_repo = DomainPolicyRepository::new(pool);

    for item in items {
        // 1. Atomic O_EXCL claim — one notification per item per cadence
        //    window. A second fire inside the window collides on the same
        //    filename and is suppressed.
        if cfg.claim && !try_claim(&cfg.claims_dir, &item, now) {
            out.suppressed.push((item.rule.clone(), item.dedup_key));
            continue;
        }

        // 2. User-visible channel — every claimed item.
        out.user_visible.push(item.clone());

        // 3. Agent channel — ONLY explicit opt-outs. The default
        //    (user_visible_only = true) never reaches injected context.
        if !item.user_visible_only {
            out.agent_visible.push(item.clone());
        }

        // 4. Obsidian path — per-item obsidian_mirror consult, FAIL-CLOSED.
        match policy_repo.get(&item.domain_key).await {
            Err(e) => {
                let reason = format!("policy lookup error for domain '{}'", item.domain_key);
                audit_drop(pool, &item, "notify_policy_drop", &reason, Some(&e.to_string())).await;
                out.dropped.push((item.rule.clone(), reason));
            }
            Ok(None) => {
                let reason = format!("no domain policy for '{}'", item.domain_key);
                audit_drop(pool, &item, "notify_policy_drop", &reason, None).await;
                out.dropped.push((item.rule.clone(), reason));
            }
            Ok(Some(policy)) => {
                if policy.obsidian_mirror == "never" {
                    // Policy decision (not an error): never mirrored to a
                    // syncable vault. Stays user-visible locally.
                    audit_drop(
                        pool,
                        &item,
                        "notify_policy_denied",
                        "obsidian_mirror=never",
                        None,
                    )
                    .await;
                    out.obsidian_blocked.push(item);
                } else {
                    // opt_in / default_on — the daily brief IS the opted-in
                    // surface (same posture as core::mirror for sub-
                    // confidential business content).
                    out.obsidian.push(item);
                }
            }
        }
    }
    Ok(out)
}

/// `O_CREAT|O_EXCL` claim. Returns false when the claim already exists
/// (= already notified within this cadence window).
fn try_claim(dir: &std::path::Path, item: &NotifyItem, now: DateTime<Utc>) -> bool {
    let path = dir.join(claim_filename(item, now));
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true) // O_CREAT | O_EXCL
        .open(&path)
    {
        Ok(_) => true,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(e) => {
            // Unexpected FS error → suppress (fail-closed for noise) + log.
            tracing::warn!("notify claim failed for {}: {e}", path.display());
            false
        }
    }
}

/// `{rule}-{sha256[:12](dedup_key)}-{bucket}.claim` — the dedup key is
/// hashed so names/ids never appear in a filename.
fn claim_filename(item: &NotifyItem, now: DateTime<Utc>) -> String {
    let mut h = Sha256::new();
    h.update(item.dedup_key.as_bytes());
    let digest = hex::encode(h.finalize());
    format!(
        "{}-{}-{}.claim",
        item.rule,
        &digest[..12],
        cadence_bucket(&item.rule, now)
    )
}

/// Append an audit_log row for a policy drop/denial. Details carry ONLY
/// rule/domain/reason metadata — never the item title/body (a relationship
/// title contains a name).
async fn audit_drop(
    pool: &SqlitePool,
    item: &NotifyItem,
    action: &str,
    reason: &str,
    error: Option<&str>,
) {
    let details = serde_json::json!({
        "rule": item.rule,
        "domain_key": item.domain_key,
        "reason": reason,
        "error": error,
        "channel": "obsidian",
    });
    let res = sqlx::query(
        "INSERT INTO audit_log (id, action, subject_type, subject_id, actor, details) \
         VALUES (?, ?, 'notify_rule', ?, 'system:notify', ?)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(action)
    .bind(&item.rule)
    .bind(details.to_string())
    .execute(pool)
    .await;
    if let Err(e) = res {
        tracing::warn!("notify audit row failed: {e}");
    }
}

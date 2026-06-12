//! Tool Register repository (PLAN-ALIVE §P1) — `tool_records` (migration 036).
//!
//! Invocable tools are distinct from AI-agent adapters (023). The persistence
//! boundary enforces the two load-bearing §P1 guarantees:
//!  - **Identity:** a tool is keyed by `(name, kind)` — never name alone
//!    ("codex" is legitimately a skill AND a binary). Upsert merges, never
//!    duplicates.
//!  - **Security (mandatory):** EVERY field passes the guard at upsert
//!    (`guard_text` for strings, recursive guard for JSON values) — capability
//!    YAMLs and documented invocations routinely embed bearer tokens; one raw
//!    credential here would fan out (DB → SessionStart injection → re-recorded
//!    into turns → served over MCP). Detections land in `secret_sightings`
//!    (fingerprint only, NEVER the value) keyed by `tool_record:{name}/{kind}`.

use altevra_core::security::Sensitivity;
use altevra_secrets::{guard_text, SecretSighting};
use chrono::Utc;
use sqlx::{Row, SqlitePool};

use crate::util::ts_to_text;

/// The allowed `kind` discriminants (locked in §P1.1).
pub const TOOL_KINDS: &[&str] = &[
    "skill",
    "cli",
    "python-api",
    "mcp-server",
    "web-service",
    "adb",
    "binary",
    // E1 (PLAN-EXTEND): an external-tool connector (ICS/IMAP/Linear/Obsidian)
    // is a tool too — registered with source=manual so it surfaces in `tool list`.
    "connector",
];

/// The allowed verification statuses (honest can/cannot/unverified ladder).
pub const TOOL_STATUSES: &[&str] = &["can", "cannot", "unverified"];

#[derive(Debug, Clone)]
pub struct ToolRecordRow {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    /// JSON: `{"canonical": "...", "alternates": ["..."]}`.
    pub invocation: serde_json::Value,
    /// JSON array of every discovered install location.
    pub locations: serde_json::Value,
    pub can_do: serde_json::Value,
    pub cannot_do: serde_json::Value,
    pub unverified: serde_json::Value,
    pub requires_session: serde_json::Value,
    /// can | cannot | unverified.
    pub status: String,
    pub last_verified_at: Option<String>,
    pub categories: serde_json::Value,
    /// scan | hook | manual.
    pub source: String,
    /// Link-by-name to `adapter_dossiers.tool_name` (hermes/codex/cursor live
    /// in both worlds). No SQL FK — either side may be seeded first.
    pub adapter_ref: Option<String>,
}

impl ToolRecordRow {
    /// A minimal row with sane defaults — callers fill what they discovered.
    pub fn new(name: &str, kind: &str) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            kind: kind.to_string(),
            display_name: None,
            description: None,
            invocation: serde_json::json!({}),
            locations: serde_json::json!([]),
            can_do: serde_json::json!([]),
            cannot_do: serde_json::json!([]),
            unverified: serde_json::json!([]),
            requires_session: serde_json::json!({}),
            status: "unverified".to_string(),
            last_verified_at: None,
            categories: serde_json::json!(["tool"]),
            source: "scan".to_string(),
            adapter_ref: None,
        }
    }
}

/// Recursively scrub every string leaf of a JSON value through `guard_text`.
/// Mirrors the CLI hook path's `guard_json` (hook_handle.rs) — re-implemented
/// here because the persistence boundary must not trust callers to pre-guard.
pub(crate) fn guard_value(v: &serde_json::Value) -> (serde_json::Value, Vec<SecretSighting>) {
    match v {
        serde_json::Value::String(s) => {
            let g = guard_text(s, Sensitivity::Internal);
            (serde_json::Value::String(g.value), g.sightings)
        }
        serde_json::Value::Array(arr) => {
            let mut out = Vec::with_capacity(arr.len());
            let mut sightings = Vec::new();
            for item in arr {
                let (vv, s) = guard_value(item);
                out.push(vv);
                sightings.extend(s);
            }
            (serde_json::Value::Array(out), sightings)
        }
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            let mut sightings = Vec::new();
            for (k, vv) in map {
                let (rv, s) = guard_value(vv);
                out.insert(k.clone(), rv);
                sightings.extend(s);
            }
            (serde_json::Value::Object(out), sightings)
        }
        other => (other.clone(), Vec::new()),
    }
}

/// Guard an optional plain-text field; collects sightings into `acc`.
pub(crate) fn guard_opt(s: &Option<String>, acc: &mut Vec<SecretSighting>) -> Option<String> {
    s.as_ref().map(|raw| {
        let g = guard_text(raw, Sensitivity::Internal);
        acc.extend(g.sightings);
        g.value
    })
}

/// Persist secret sightings (fingerprint + metadata ONLY, never the value).
/// `INSERT OR IGNORE` on `UNIQUE(fingerprint, source_ref)` keeps re-scans
/// idempotent.
pub(crate) async fn record_sightings(
    pool: &SqlitePool,
    sightings: &[SecretSighting],
    source_ref: &str,
    location: &str,
) -> anyhow::Result<usize> {
    for s in sightings {
        sqlx::query(
            "INSERT OR IGNORE INTO secret_sightings \
             (id, secret_kind, fingerprint, source_ref, location, action) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(&s.secret_kind)
        .bind(&s.fingerprint)
        .bind(source_ref)
        .bind(location)
        .bind(&s.action)
        .execute(pool)
        .await?;
    }
    Ok(sightings.len())
}

pub struct ToolRecordsRepository<'a> {
    pool: &'a SqlitePool,
}

impl<'a> ToolRecordsRepository<'a> {
    pub fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Upsert by `(name, kind)`. Every field is guarded HERE (not trusted from
    /// the caller); detections are recorded in `secret_sightings`. Returns the
    /// number of secret sightings recorded for this row.
    pub async fn upsert(&self, row: &ToolRecordRow) -> anyhow::Result<usize> {
        if !TOOL_KINDS.contains(&row.kind.as_str()) {
            anyhow::bail!(
                "tool_record kind '{}' is not one of {TOOL_KINDS:?}",
                row.kind
            );
        }
        if !TOOL_STATUSES.contains(&row.status.as_str()) {
            anyhow::bail!(
                "tool_record status '{}' is not one of {TOOL_STATUSES:?}",
                row.status
            );
        }
        if !["scan", "hook", "manual"].contains(&row.source.as_str()) {
            anyhow::bail!(
                "tool_record source '{}' is not one of [scan, hook, manual]",
                row.source
            );
        }

        // ---- guard every field (mandatory §P1.3) ----
        let mut sightings: Vec<SecretSighting> = Vec::new();
        let g_name = {
            let g = guard_text(&row.name, Sensitivity::Internal);
            sightings.extend(g.sightings);
            g.value
        };
        let g_display = guard_opt(&row.display_name, &mut sightings);
        let g_desc = guard_opt(&row.description, &mut sightings);
        let g_adapter = guard_opt(&row.adapter_ref, &mut sightings);
        let (g_invocation, s) = guard_value(&row.invocation);
        sightings.extend(s);
        let (g_locations, s) = guard_value(&row.locations);
        sightings.extend(s);
        let (g_can, s) = guard_value(&row.can_do);
        sightings.extend(s);
        let (g_cannot, s) = guard_value(&row.cannot_do);
        sightings.extend(s);
        let (g_unverified, s) = guard_value(&row.unverified);
        sightings.extend(s);
        let (g_requires, s) = guard_value(&row.requires_session);
        sightings.extend(s);
        let (g_categories, s) = guard_value(&row.categories);
        sightings.extend(s);

        let now = ts_to_text(&Utc::now());
        sqlx::query(
            "INSERT INTO tool_records \
             (id, name, kind, display_name, description, invocation, locations, \
              can_do, cannot_do, unverified, requires_session, status, \
              last_verified_at, categories, source, adapter_ref, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(name, kind) DO UPDATE SET \
               display_name=excluded.display_name, description=excluded.description, \
               invocation=excluded.invocation, locations=excluded.locations, \
               can_do=excluded.can_do, cannot_do=excluded.cannot_do, \
               unverified=excluded.unverified, requires_session=excluded.requires_session, \
               status=excluded.status, last_verified_at=excluded.last_verified_at, \
               categories=excluded.categories, source=excluded.source, \
               adapter_ref=excluded.adapter_ref, updated_at=excluded.updated_at",
        )
        .bind(&row.id)
        .bind(&g_name)
        .bind(&row.kind)
        .bind(g_display.as_deref())
        .bind(g_desc.as_deref())
        .bind(g_invocation.to_string())
        .bind(g_locations.to_string())
        .bind(g_can.to_string())
        .bind(g_cannot.to_string())
        .bind(g_unverified.to_string())
        .bind(g_requires.to_string())
        .bind(&row.status)
        .bind(row.last_verified_at.as_deref())
        .bind(g_categories.to_string())
        .bind(&row.source)
        .bind(g_adapter.as_deref())
        .bind(&now)
        .bind(&now)
        .execute(self.pool)
        .await?;

        // Record sightings (fingerprints only) for audit; idempotent on
        // (fingerprint, source_ref).
        let source_ref = format!("tool_record:{}/{}", g_name, row.kind);
        record_sightings(self.pool, &sightings, &source_ref, "tool_record_fields").await?;
        Ok(sightings.len())
    }

    pub async fn get(&self, name: &str, kind: &str) -> anyhow::Result<Option<ToolRecordRow>> {
        let row = sqlx::query(&format!(
            "SELECT {COLS} FROM tool_records WHERE name = ? AND kind = ?"
        ))
        .bind(name)
        .bind(kind)
        .fetch_optional(self.pool)
        .await?;
        Ok(row.map(row_to_tool))
    }

    /// All kinds carrying this name (e.g. "codex" → skill + binary rows).
    pub async fn get_by_name(&self, name: &str) -> anyhow::Result<Vec<ToolRecordRow>> {
        let rows = sqlx::query(&format!(
            "SELECT {COLS} FROM tool_records WHERE name = ? ORDER BY kind"
        ))
        .bind(name)
        .fetch_all(self.pool)
        .await?;
        Ok(rows.into_iter().map(row_to_tool).collect())
    }

    /// List, optionally filtered by kind and/or status. Manual/seeded entries
    /// first (they carry curated invocations), then by name.
    pub async fn list(
        &self,
        kind: Option<&str>,
        status: Option<&str>,
    ) -> anyhow::Result<Vec<ToolRecordRow>> {
        let order = "ORDER BY (source != 'manual'), name, kind";
        let rows = match (kind, status) {
            (Some(k), Some(s)) => {
                sqlx::query(&format!(
                    "SELECT {COLS} FROM tool_records WHERE kind = ? AND status = ? {order}"
                ))
                .bind(k)
                .bind(s)
                .fetch_all(self.pool)
                .await?
            }
            (Some(k), None) => {
                sqlx::query(&format!(
                    "SELECT {COLS} FROM tool_records WHERE kind = ? {order}"
                ))
                .bind(k)
                .fetch_all(self.pool)
                .await?
            }
            (None, Some(s)) => {
                sqlx::query(&format!(
                    "SELECT {COLS} FROM tool_records WHERE status = ? {order}"
                ))
                .bind(s)
                .fetch_all(self.pool)
                .await?
            }
            (None, None) => {
                sqlx::query(&format!("SELECT {COLS} FROM tool_records {order}"))
                    .fetch_all(self.pool)
                    .await?
            }
        };
        Ok(rows.into_iter().map(row_to_tool).collect())
    }

    /// `altevra tool verify` — set the honest status, stamping
    /// `last_verified_at`. Returns false when no such (name, kind) row exists.
    pub async fn set_status(
        &self,
        name: &str,
        kind: &str,
        status: &str,
    ) -> anyhow::Result<bool> {
        if !TOOL_STATUSES.contains(&status) {
            anyhow::bail!("status '{status}' is not one of {TOOL_STATUSES:?}");
        }
        let now = ts_to_text(&Utc::now());
        let res = sqlx::query(
            "UPDATE tool_records SET status = ?, last_verified_at = ?, updated_at = ? \
             WHERE name = ? AND kind = ?",
        )
        .bind(status)
        .bind(&now)
        .bind(&now)
        .bind(name)
        .bind(kind)
        .execute(self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }
}

const COLS: &str = "id, name, kind, display_name, description, invocation, locations, \
                    can_do, cannot_do, unverified, requires_session, status, \
                    last_verified_at, categories, source, adapter_ref";

fn json_col(r: &sqlx::sqlite::SqliteRow, col: &str, default: serde_json::Value) -> serde_json::Value {
    serde_json::from_str(&r.get::<String, _>(col)).unwrap_or(default)
}

fn row_to_tool(r: sqlx::sqlite::SqliteRow) -> ToolRecordRow {
    ToolRecordRow {
        id: r.get("id"),
        name: r.get("name"),
        kind: r.get("kind"),
        display_name: r.get("display_name"),
        description: r.get("description"),
        invocation: json_col(&r, "invocation", serde_json::json!({})),
        locations: json_col(&r, "locations", serde_json::json!([])),
        can_do: json_col(&r, "can_do", serde_json::json!([])),
        cannot_do: json_col(&r, "cannot_do", serde_json::json!([])),
        unverified: json_col(&r, "unverified", serde_json::json!([])),
        requires_session: json_col(&r, "requires_session", serde_json::json!({})),
        status: r.get("status"),
        last_verified_at: r.get("last_verified_at"),
        categories: json_col(&r, "categories", serde_json::json!(["tool"])),
        source: r.get("source"),
        adapter_ref: r.get("adapter_ref"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::{create_pool, run_migrations};

    async fn pool() -> SqlitePool {
        let p = create_pool("sqlite::memory:").await.unwrap();
        run_migrations(&p).await.unwrap();
        p
    }

    #[tokio::test]
    async fn round_trip_upsert_get_list() {
        let p = pool().await;
        let repo = ToolRecordsRepository::new(&p);
        let mut row = ToolRecordRow::new("imperium-crawl", "cli");
        row.description = Some("browser automation CLI".into());
        row.invocation = serde_json::json!({"canonical": "imperium-crawl <cmd>"});
        row.locations = serde_json::json!(["/home/x/.npm-global/bin/imperium-crawl"]);
        row.status = "can".into();
        row.source = "manual".into();
        repo.upsert(&row).await.unwrap();

        let got = repo.get("imperium-crawl", "cli").await.unwrap().unwrap();
        assert_eq!(got.status, "can");
        assert_eq!(got.invocation["canonical"], "imperium-crawl <cmd>");
        assert_eq!(got.locations.as_array().unwrap().len(), 1);

        // Upsert again with a 2nd location merges into the SAME row.
        let mut row2 = got.clone();
        row2.locations =
            serde_json::json!(["/home/x/.npm-global/bin/imperium-crawl", "/home/x/projekti/ic"]);
        repo.upsert(&row2).await.unwrap();
        let all = repo.list(Some("cli"), None).await.unwrap();
        assert_eq!(all.len(), 1, "(name,kind) upsert must merge, not duplicate");
        assert_eq!(all[0].locations.as_array().unwrap().len(), 2);

        // list filters
        assert_eq!(repo.list(None, Some("can")).await.unwrap().len(), 1);
        assert_eq!(repo.list(None, Some("cannot")).await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn same_name_different_kind_is_two_rows() {
        let p = pool().await;
        let repo = ToolRecordsRepository::new(&p);
        repo.upsert(&ToolRecordRow::new("codex", "skill")).await.unwrap();
        repo.upsert(&ToolRecordRow::new("codex", "binary")).await.unwrap();
        let rows = repo.get_by_name("codex").await.unwrap();
        assert_eq!(rows.len(), 2, "codex is legitimately a skill AND a binary");
        let kinds: Vec<_> = rows.iter().map(|r| r.kind.as_str()).collect();
        assert!(kinds.contains(&"skill") && kinds.contains(&"binary"));
    }

    #[tokio::test]
    async fn invalid_kind_and_status_rejected() {
        let p = pool().await;
        let repo = ToolRecordsRepository::new(&p);
        assert!(repo.upsert(&ToolRecordRow::new("x", "wasm")).await.is_err());
        let mut bad = ToolRecordRow::new("x", "cli");
        bad.status = "maybe".into();
        assert!(repo.upsert(&bad).await.is_err());
        assert!(repo.set_status("x", "cli", "maybe").await.is_err());
    }

    #[tokio::test]
    async fn embedded_secret_is_redacted_and_sighting_logged() {
        // §P1.3 (mandatory): a documented invocation embedding a bearer token
        // must NEVER persist raw — redacted in the row + a fingerprint-only
        // sighting in secret_sightings.
        let p = pool().await;
        let repo = ToolRecordsRepository::new(&p);
        let mut row = ToolRecordRow::new("evil-svc", "web-service");
        row.invocation = serde_json::json!({
            "canonical": "curl -H 'Authorization: Bearer abcdefghijklmnop0123456789' http://x"
        });
        row.description = Some("uses key sk-FIXTUREfixtureFIXTUREfixture0000 inline".into());
        let n = repo.upsert(&row).await.unwrap();
        assert!(n >= 2, "both the bearer token and the api key must be sighted");

        let got = repo.get("evil-svc", "web-service").await.unwrap().unwrap();
        let inv = got.invocation.to_string();
        assert!(!inv.contains("abcdefghijklmnop0123456789"), "token leaked: {inv}");
        assert!(inv.contains("[REDACTED]"));
        assert!(!got.description.as_deref().unwrap().contains("sk-FIXTURE"));

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM secret_sightings WHERE source_ref = 'tool_record:evil-svc/web-service'",
        )
        .fetch_one(&p)
        .await
        .unwrap();
        assert!(count >= 2, "sightings must be logged, got {count}");
        // Never the raw value in the sightings table either.
        let any_raw: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM secret_sightings WHERE fingerprint LIKE '%abcdefghijklmnop%'",
        )
        .fetch_one(&p)
        .await
        .unwrap();
        assert_eq!(any_raw, 0);

        // Idempotent re-upsert: same fingerprints, no duplicate sighting rows.
        repo.upsert(&row).await.unwrap();
        let count2: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM secret_sightings WHERE source_ref = 'tool_record:evil-svc/web-service'",
        )
        .fetch_one(&p)
        .await
        .unwrap();
        assert_eq!(count, count2, "re-scan must not duplicate sightings");
    }

    #[tokio::test]
    async fn verify_sets_status_and_timestamp() {
        let p = pool().await;
        let repo = ToolRecordsRepository::new(&p);
        repo.upsert(&ToolRecordRow::new("phone-use", "adb")).await.unwrap();
        assert!(repo.set_status("phone-use", "adb", "can").await.unwrap());
        let got = repo.get("phone-use", "adb").await.unwrap().unwrap();
        assert_eq!(got.status, "can");
        assert!(got.last_verified_at.is_some());
        // Unknown row → false, not error.
        assert!(!repo.set_status("ghost", "cli", "can").await.unwrap());
    }
}

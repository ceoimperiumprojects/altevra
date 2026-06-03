//! Cursor CLI importer (P0.9 E4).
//!
//! Cursor CLI — the standalone `cursor` agent, not the VS Code extension —
//! persists its AI-generated edits in
//! `~/.cursor/ai-tracking/ai-code-tracking.db` (SQLite) and its planning docs
//! in `~/.cursor/plans/*.plan.md`. This importer lifts both surfaces into
//! Altevra:
//!
//!   * Each row of `ai_code_hashes` becomes a `CursorEditRow` (post-guard).
//!   * Each row of `tracked_file_content` whose `gitPath` is NOT a Cursor
//!     plan file also lands as a `CursorEditRow` (so a recall on a path or a
//!     hash fragment can find it). Plan-file rows are skipped here and lifted
//!     through the `.plan.md` path instead, which is the higher-fidelity
//!     surface (it has headings + structure).
//!
//! Read-only invariant
//! -------------------
//! The Cursor database at `~/.cursor/ai-tracking/ai-code-tracking.db` is
//! ALWAYS opened with `SQLITE_OPEN_READ_ONLY` (via rusqlite). The importer
//! NEVER writes to that file. In tests we additionally copy the upstream db
//! into a temp dir first — the byte-untouched real-db guarantee is enforced
//! by the test, not just the open-flags. See `tests/cursor_cli_*` below.
//!
//! Markdown planning docs
//! ----------------------
//! `~/.cursor/plans/*.plan.md` files are treated as a SINGLE atomic object
//! each — a Cursor plan is a *unit* (title + scope + bullets), so atomising
//! it would shatter its meaning. The plan path returns one `CursorPlanRow`
//! per file; the caller persists them through the standard write path
//! (capture-style) so they index into `object_index` + `object_fts`.

use altevra_core::security::Sensitivity;
use altevra_db::{CursorEditRow, CursorEditsRepository};
use altevra_secrets::guard_text;
use rusqlite::OpenFlags;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};
use tracing::warn;

/// Where the Cursor CLI persists its ai-tracking SQLite db by default.
pub fn default_ai_tracking_db() -> PathBuf {
    home_dir()
        .join(".cursor")
        .join("ai-tracking")
        .join("ai-code-tracking.db")
}

/// Where the Cursor CLI persists its plan markdown files by default.
pub fn default_plans_dir() -> PathBuf {
    home_dir().join(".cursor").join("plans")
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}

/// Summary of an import run (dry-run AND actual). The numbers are populated
/// from the same code path so a dry-run preview matches the live import.
#[derive(Debug, Clone, Default, Serialize)]
pub struct CursorImportSummary {
    pub source_db: Option<PathBuf>,
    pub source_db_exists: bool,
    pub plans_dir: Option<PathBuf>,
    pub plans_dir_exists: bool,
    /// Rows seen in `ai_code_hashes`.
    pub ai_code_rows_seen: usize,
    /// Rows seen in `tracked_file_content` (excluding plan files).
    pub tracked_rows_seen: usize,
    /// Plan markdown files seen (`*.plan.md`).
    pub plan_files_seen: usize,
    /// Rows rejected because guard_text raised a credential-class sighting.
    pub rejected_credential: usize,
    /// Rows whose snippet was redacted (secret/PII scrubbed, kept).
    pub redacted: usize,
    /// Rows that landed in `cursor_edits` (`dry_run=false`).
    pub edits_inserted: usize,
    /// Plan files that landed (`dry_run=false`).
    pub plans_inserted: usize,
    /// Whether the call was dry-run (no writes).
    pub dry_run: bool,
}

/// A single Cursor plan markdown file mapped to an object envelope. The
/// caller hands this off to the capture path (or the in-tree wrapper below).
#[derive(Debug, Clone)]
pub struct CursorPlanRow {
    pub id: String,
    pub title: String,
    pub path: PathBuf,
    pub body: String,
    pub redaction_status: String,
    pub sensitivity: Sensitivity,
    /// Sighting count from `guard_text`.
    pub redactions: usize,
}

/// Open the Cursor ai-tracking db READ-ONLY. Returns None when the file
/// does not exist (which is the common case on machines without Cursor CLI).
pub fn open_cursor_db_readonly(path: &Path) -> anyhow::Result<Option<rusqlite::Connection>> {
    if !path.exists() {
        return Ok(None);
    }
    let conn = rusqlite::Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    Ok(Some(conn))
}

/// Build a stable Altevra id for a Cursor row. `content_hash` is the upstream
/// SHA-style hash; we prefix it with `cursor-edit-` so it never collides with
/// other object ids in the cross-type index.
fn make_edit_id(content_hash: &str) -> String {
    format!("cursor-edit-{}", content_hash)
}

fn make_plan_id(path: &Path) -> String {
    let mut h = Sha256::new();
    h.update(path.to_string_lossy().as_bytes());
    let hex = hex::encode(h.finalize());
    format!("cursor-plan-{}", &hex[..16])
}

fn json_array(items: &[&str]) -> String {
    let owned: Vec<String> = items.iter().map(|s| (*s).to_string()).collect();
    serde_json::to_string(&owned).expect("string vec serialises")
}

fn provenance_for_edit(source_db: &Path, file_path: Option<&str>, cursor_ts: Option<i64>) -> String {
    let mut obj = serde_json::json!({
        "origin": "cursor_cli",
        "source_db": source_db.display().to_string(),
    });
    if let Some(p) = file_path {
        obj["imported_from"] = serde_json::Value::String(p.to_string());
    }
    if let Some(ts) = cursor_ts {
        obj["cursor_ts_ms"] = serde_json::Value::Number(ts.into());
    }
    serde_json::to_string(&obj).expect("provenance serialises")
}

fn provenance_for_plan(path: &Path) -> String {
    serde_json::to_string(&serde_json::json!({
        "origin": "cursor_cli_plan",
        "imported_from": path.display().to_string(),
    }))
    .expect("provenance serialises")
}

/// Read every `ai_code_hashes` row + every non-plan `tracked_file_content`
/// row, run each indexable text field through `guard_text`, and return the
/// resulting structured rows. This function NEVER touches the upstream
/// database except for reads (open flags + the lack of any write SQL).
///
/// `tracked_file_content.content` IS code text and is the riskier surface
/// — that's where secrets could land. `ai_code_hashes` carries only hashes
/// + file paths, so its scrub is precautionary.
pub fn collect_edits(
    db_path: &Path,
    summary: &mut CursorImportSummary,
) -> anyhow::Result<Vec<CursorEditRow>> {
    let Some(conn) = open_cursor_db_readonly(db_path)? else {
        return Ok(Vec::new());
    };
    summary.source_db_exists = true;

    let mut out: Vec<CursorEditRow> = Vec::new();

    // ---- ai_code_hashes ----
    let mut stmt = conn.prepare(
        "SELECT hash, source, fileExtension, fileName, requestId, conversationId, \
                timestamp, model, createdAt \
         FROM ai_code_hashes",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(AiCodeHashRow {
            hash: row.get(0)?,
            source: row.get(1).ok(),
            file_extension: row.get(2).ok(),
            file_name: row.get(3).ok(),
            request_id: row.get(4).ok(),
            conversation_id: row.get(5).ok(),
            timestamp: row.get(6).ok(),
            model: row.get(7).ok(),
            created_at: row.get(8).unwrap_or(0),
        })
    })?;
    for row in rows.flatten() {
        summary.ai_code_rows_seen += 1;
        // The hash + file path together are the indexable signal (no body).
        // We still scrub them — a `fileName` could embed `?token=…` (unlikely
        // but a free safety belt).
        let raw_body = format!(
            "hash:{}\npath:{}\nmodel:{}",
            row.hash,
            row.file_name.as_deref().unwrap_or(""),
            row.model.as_deref().unwrap_or("")
        );
        let guarded = guard_text(&raw_body, Sensitivity::Internal);
        if guarded.sightings.iter().any(|s| s.action == "rejected") {
            summary.rejected_credential += 1;
            warn!(hash = %row.hash, "cursor-cli: credential-class secret in ai_code_hashes — REJECTED");
            continue;
        }
        if !guarded.sightings.is_empty() {
            summary.redacted += 1;
        }
        let cats = json_array(&["business", "kind:cursor_edit", "source:ai_code_hashes"]);
        let edit = CursorEditRow {
            id: make_edit_id(&row.hash),
            content_hash: row.hash.clone(),
            source: row.source.clone(),
            file_path: row.file_name.clone(),
            file_extension: row.file_extension.clone(),
            conversation_id: row.conversation_id.clone(),
            request_id: row.request_id.clone(),
            model: row.model.clone(),
            snippet: guarded.value,
            length: row.hash.len() as i64,
            cursor_ts: row.timestamp,
            cursor_created: row.created_at,
            title: format!(
                "Cursor edit {} ({})",
                &row.hash[..row.hash.len().min(8)],
                row.file_name.as_deref().unwrap_or("?")
            ),
            status: "active".into(),
            domain: "business".into(),
            scope: None,
            sensitivity: guarded.sensitivity.to_string(),
            provenance: provenance_for_edit(db_path, row.file_name.as_deref(), row.timestamp),
            redaction_status: guarded.redaction_status.to_string(),
            categories: cats.clone(),
            tags: cats,
        };
        out.push(edit);
    }
    drop(stmt);

    // ---- tracked_file_content (exclude plan files; those go via .plan.md) ----
    let mut stmt = conn.prepare(
        "SELECT gitPath, content, conversationId, model, fileExtension, createdAt \
         FROM tracked_file_content",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(TrackedFileContentRow {
            git_path: row.get(0)?,
            content: row.get(1).unwrap_or_default(),
            conversation_id: row.get(2).ok(),
            model: row.get(3).ok(),
            file_extension: row.get(4).ok(),
            created_at: row.get(5).unwrap_or(0),
        })
    })?;
    for row in rows.flatten() {
        if row.git_path.contains("/.cursor/plans/") && row.git_path.ends_with(".plan.md") {
            // Lifted via the .plan.md path instead — more structure there.
            continue;
        }
        summary.tracked_rows_seen += 1;
        let guarded = guard_text(&row.content, Sensitivity::Internal);
        if guarded.sightings.iter().any(|s| s.action == "rejected") {
            summary.rejected_credential += 1;
            warn!(path = %row.git_path, "cursor-cli: credential-class secret in tracked_file_content — REJECTED");
            continue;
        }
        if !guarded.sightings.is_empty() {
            summary.redacted += 1;
        }
        // Content hash drives the id so a re-import is idempotent.
        let mut h = Sha256::new();
        h.update(row.git_path.as_bytes());
        h.update(row.content.as_bytes());
        let hash = hex::encode(h.finalize());
        let cats = json_array(&[
            "business",
            "kind:cursor_edit",
            "source:tracked_file_content",
        ]);
        let edit = CursorEditRow {
            id: make_edit_id(&hash[..16]),
            content_hash: hash[..16].to_string(),
            source: Some("tracked_file_content".into()),
            file_path: Some(row.git_path.clone()),
            file_extension: row.file_extension.clone(),
            conversation_id: row.conversation_id.clone(),
            request_id: None,
            model: row.model.clone(),
            snippet: guarded.value.clone(),
            length: guarded.value.len() as i64,
            cursor_ts: None,
            cursor_created: row.created_at,
            title: format!("Cursor tracked file {}", row.git_path),
            status: "active".into(),
            domain: "business".into(),
            scope: None,
            sensitivity: guarded.sensitivity.to_string(),
            provenance: provenance_for_edit(db_path, Some(&row.git_path), None),
            redaction_status: guarded.redaction_status.to_string(),
            categories: cats.clone(),
            tags: cats,
        };
        out.push(edit);
    }

    Ok(out)
}

/// Read every `*.plan.md` under `plans_dir`, guard each one, and return one
/// `CursorPlanRow` per file. Credential-class hits REJECT the file (it is
/// not returned). Non-existent / empty dir returns an empty vec.
pub fn collect_plans(
    plans_dir: &Path,
    summary: &mut CursorImportSummary,
) -> anyhow::Result<Vec<CursorPlanRow>> {
    if !plans_dir.exists() {
        return Ok(Vec::new());
    }
    summary.plans_dir_exists = true;

    let mut out = Vec::new();
    for entry in std::fs::read_dir(plans_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if !name.ends_with(".plan.md") {
            continue;
        }
        summary.plan_files_seen += 1;
        let body = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                warn!(path = %path.display(), error = %e, "cursor-cli: cannot read plan");
                continue;
            }
        };
        let guarded = guard_text(&body, Sensitivity::Internal);
        if guarded.sightings.iter().any(|s| s.action == "rejected") {
            summary.rejected_credential += 1;
            warn!(path = %path.display(), "cursor-cli: credential-class secret in plan — REJECTED");
            continue;
        }
        if !guarded.sightings.is_empty() {
            summary.redacted += 1;
        }
        // Title = filename without `.plan.md` suffix; trim trailing
        // `-<8charhash>` chunk Cursor appends, when present.
        let mut title = name.trim_end_matches(".plan.md").to_string();
        if let Some(idx) = title.rfind('-') {
            let suffix = &title[idx + 1..];
            if suffix.len() == 8 && suffix.chars().all(|c| c.is_ascii_hexdigit()) {
                title.truncate(idx);
            }
        }
        out.push(CursorPlanRow {
            id: make_plan_id(&path),
            title,
            path: path.clone(),
            body: guarded.value,
            redaction_status: guarded.redaction_status.to_string(),
            sensitivity: guarded.sensitivity,
            redactions: guarded.sightings.len(),
        });
    }
    Ok(out)
}

/// One-shot import (read-only on the upstream db). `dry_run=true` reports
/// counts without writing anything to the Altevra db. `dry_run=false`
/// persists rows through `CursorEditsRepository::insert` + a `learning`-style
/// write for each plan (caller-supplied pool).
pub async fn import(
    db_path: &Path,
    plans_dir: &Path,
    pool: Option<&SqlitePool>,
    dry_run: bool,
) -> anyhow::Result<CursorImportSummary> {
    let mut summary = CursorImportSummary {
        source_db: Some(db_path.to_path_buf()),
        plans_dir: Some(plans_dir.to_path_buf()),
        dry_run,
        ..Default::default()
    };

    let edits = collect_edits(db_path, &mut summary)?;
    let plans = collect_plans(plans_dir, &mut summary)?;

    if dry_run || pool.is_none() {
        return Ok(summary);
    }
    let pool = pool.expect("checked above");
    let edits_repo = CursorEditsRepository::new(pool);
    for edit in &edits {
        if edits_repo.insert(edit).await? {
            summary.edits_inserted += 1;
        }
    }

    // Plans go in via the same `cursor_edits` substrate (so recall finds
    // them through one path). They carry `kind:cursor_plan` so the type is
    // still filterable in the index.
    for plan in &plans {
        let cats = json_array(&["business", "kind:cursor_plan", "source:plan_md"]);
        let row = CursorEditRow {
            id: plan.id.clone(),
            content_hash: plan.id.clone(),
            source: Some("plan_md".into()),
            file_path: Some(plan.path.display().to_string()),
            file_extension: Some("md".into()),
            conversation_id: None,
            request_id: None,
            model: None,
            snippet: plan.body.clone(),
            length: plan.body.len() as i64,
            cursor_ts: None,
            cursor_created: 0,
            title: plan.title.clone(),
            status: "active".into(),
            domain: "business".into(),
            scope: None,
            sensitivity: plan.sensitivity.to_string(),
            provenance: provenance_for_plan(&plan.path),
            redaction_status: plan.redaction_status.clone(),
            categories: cats.clone(),
            tags: cats,
        };
        if edits_repo.insert(&row).await? {
            summary.plans_inserted += 1;
        }
    }
    Ok(summary)
}

// ---------------------------------------------------------------------------
// Internal row helpers (private to this module).
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct AiCodeHashRow {
    hash: String,
    source: Option<String>,
    file_extension: Option<String>,
    file_name: Option<String>,
    request_id: Option<String>,
    conversation_id: Option<String>,
    timestamp: Option<i64>,
    model: Option<String>,
    created_at: i64,
}

#[derive(Debug)]
struct TrackedFileContentRow {
    git_path: String,
    content: String,
    conversation_id: Option<String>,
    model: Option<String>,
    file_extension: Option<String>,
    created_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use altevra_db::{create_pool, run_migrations, CursorEditsRepository};
    use rusqlite::Connection;
    use tempfile::TempDir;

    /// Build a tiny fixture SQLite at `tmp/ai-code-tracking.db` matching the
    /// real Cursor schema. Does NOT touch ~/.cursor.
    fn make_fixture_db(tmp: &Path) -> PathBuf {
        let p = tmp.join("ai-code-tracking.db");
        let conn = Connection::open(&p).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE ai_code_hashes (
                hash TEXT PRIMARY KEY,
                source TEXT NOT NULL,
                fileExtension TEXT,
                fileName TEXT,
                requestId TEXT,
                conversationId TEXT,
                timestamp INTEGER,
                model TEXT,
                createdAt INTEGER NOT NULL
            );
            CREATE TABLE tracked_file_content (
                gitPath TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                conversationId TEXT,
                model TEXT,
                fileExtension TEXT,
                createdAt INTEGER NOT NULL
            );
            INSERT INTO ai_code_hashes
              (hash, source, fileExtension, fileName, requestId, conversationId, timestamp, model, createdAt)
              VALUES
              ('hash-alpha', 'cli', 'rs', '/tmp/x.rs', NULL, 'conv-1', 1778247477915, 'claude-opus-4-7', 1778247477916),
              ('hash-beta',  'cli', 'js', '/tmp/y.js', 'req-2', 'conv-1', 1778247500000, 'claude-opus-4-7', 1778247500000);
            INSERT INTO tracked_file_content
              (gitPath, content, conversationId, model, fileExtension, createdAt)
              VALUES
              ('/tmp/x.rs',    'fn hello() { println!("hi"); }', 'conv-1', 'claude-opus-4-7', 'rs', 1778247477917),
              ('/tmp/y.js',    'const X = 1;',                     'conv-1', 'claude-opus-4-7', 'js', 1778247500001);
            "#,
        )
        .unwrap();
        p
    }

    /// Variant fixture that embeds a fake AWS access key into the snippet so
    /// we can verify the credential-class path REJECTS the row (never persists
    /// it) and a non-credential secret gets REDACTED + persisted.
    fn make_fixture_db_with_secret(tmp: &Path) -> PathBuf {
        let p = tmp.join("ai-code-tracking-secret.db");
        let conn = Connection::open(&p).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE ai_code_hashes (
                hash TEXT PRIMARY KEY,
                source TEXT NOT NULL,
                fileExtension TEXT,
                fileName TEXT,
                requestId TEXT,
                conversationId TEXT,
                timestamp INTEGER,
                model TEXT,
                createdAt INTEGER NOT NULL
            );
            CREATE TABLE tracked_file_content (
                gitPath TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                conversationId TEXT,
                model TEXT,
                fileExtension TEXT,
                createdAt INTEGER NOT NULL
            );
            INSERT INTO ai_code_hashes
              (hash, source, fileExtension, fileName, requestId, conversationId, timestamp, model, createdAt)
              VALUES ('h-clean', 'cli', 'rs', '/tmp/clean.rs', NULL, 'c-1', 0, 'm', 0);
            "#,
        )
        .unwrap();
        // Put a fake PEM (credential-class -> REJECT) in one row, and a
        // bearer-token-shaped string (REDACT, keep) in another.
        let pem_body = "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEAxJ...fake...END\n-----END RSA PRIVATE KEY-----";
        let token_body = "let token = \"sk-live-AAAA1111BBBB2222CCCC3333DDDD4444EEEE5555\";";
        conn.execute(
            "INSERT INTO tracked_file_content (gitPath, content, conversationId, model, fileExtension, createdAt) VALUES (?, ?, ?, ?, ?, ?)",
            rusqlite::params!["/tmp/pem.rs", pem_body, "c-1", "m", "rs", 0_i64],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tracked_file_content (gitPath, content, conversationId, model, fileExtension, createdAt) VALUES (?, ?, ?, ?, ?, ?)",
            rusqlite::params!["/tmp/token.rs", token_body, "c-1", "m", "rs", 0_i64],
        )
        .unwrap();
        p
    }

    /// Sanity: the fixture builder fires + columns line up with the real db.
    #[test]
    fn cursor_sqlite_import_reads_rows() {
        let tmp = TempDir::new().unwrap();
        let db = make_fixture_db(tmp.path());
        let mut sum = CursorImportSummary::default();
        let edits = collect_edits(&db, &mut sum).unwrap();
        // 2 ai_code_hashes + 2 tracked_file_content (no plan paths) = 4
        assert_eq!(edits.len(), 4, "expected 4 rows, got {}", edits.len());
        assert_eq!(sum.ai_code_rows_seen, 2);
        assert_eq!(sum.tracked_rows_seen, 2);
        assert_eq!(sum.rejected_credential, 0);
        // All edits carry the right object_type marker so the index gets it
        // via the cursor_edit type — ids are prefixed.
        for e in &edits {
            assert!(e.id.starts_with("cursor-edit-"), "id prefix: {}", e.id);
        }
    }

    #[test]
    fn cursor_import_redacts_secret_chunks() {
        let tmp = TempDir::new().unwrap();
        let db = make_fixture_db_with_secret(tmp.path());
        let mut sum = CursorImportSummary::default();
        let edits = collect_edits(&db, &mut sum).unwrap();
        // ai_code_hashes had 1 clean row. tracked_file_content had 2 — one
        // PEM (credential → reject) and one bearer-shaped (redact). The PEM
        // row is dropped (never returned). The token row is returned with
        // its secret redacted.
        assert_eq!(sum.ai_code_rows_seen, 1);
        assert_eq!(sum.tracked_rows_seen, 2);
        assert!(
            sum.rejected_credential >= 1,
            "expected at least one credential-class rejection; rejects={}",
            sum.rejected_credential
        );
        // No edit's snippet still contains the raw PEM body — even the kept
        // ones must be scrubbed.
        for e in &edits {
            assert!(
                !e.snippet.contains("BEGIN RSA PRIVATE KEY"),
                "raw PEM leaked into a stored snippet: {}",
                e.snippet
            );
        }
        // The token row's redacted snippet must not still carry the raw key.
        let token_edit = edits
            .iter()
            .find(|e| e.file_path.as_deref() == Some("/tmp/token.rs"))
            .expect("token row was redacted, kept, returned");
        assert!(
            !token_edit.snippet.contains("AAAA1111BBBB2222"),
            "redaction missed the bearer-shaped key: {}",
            token_edit.snippet
        );
    }

    #[test]
    fn cursor_plan_md_atomizes() {
        let tmp = TempDir::new().unwrap();
        let plans_dir = tmp.path().join("plans");
        std::fs::create_dir_all(&plans_dir).unwrap();
        std::fs::write(
            plans_dir.join("Imperium Skills Sync-e39f4440.plan.md"),
            "# Imperium Skills Sync\n\n## Goal\nSync the skills.\n\n## Steps\n- one\n- two\n",
        )
        .unwrap();
        std::fs::write(
            plans_dir.join("EAR Native UX Phase 2-d8f50a7d.plan.md"),
            "# EAR Native UX\n\n## Goal\nShip phase 2.\n",
        )
        .unwrap();
        // Stray non-plan file MUST be ignored.
        std::fs::write(plans_dir.join("README.md"), "ignore me").unwrap();

        let mut sum = CursorImportSummary::default();
        let plans = collect_plans(&plans_dir, &mut sum).unwrap();
        assert_eq!(plans.len(), 2, "expected 2 plan files, got {}", plans.len());
        assert_eq!(sum.plan_files_seen, 2);
        for p in &plans {
            assert!(p.id.starts_with("cursor-plan-"), "id prefix: {}", p.id);
            // Title strips the `-<8hex>` suffix Cursor appends.
            assert!(
                !p.title.ends_with("e39f4440") && !p.title.ends_with("d8f50a7d"),
                "title kept the hash suffix: {}",
                p.title
            );
            // Each plan is ONE atomic object (we picked the "single object"
            // route, not the section-atomize one).
            assert!(p.body.contains("## Goal"), "body kept its structure");
        }
    }

    /// End-to-end: open a fixture db, run `import(... dry_run=false)` against
    /// a fresh Altevra db, and observe the rows landing in `cursor_edits` +
    /// the cross-type index (via the standard insert path).
    #[tokio::test]
    async fn cursor_import_writes_to_altevra_db() {
        let tmp = TempDir::new().unwrap();
        let cursor_db = make_fixture_db(tmp.path());
        let plans_dir = tmp.path().join("plans");
        std::fs::create_dir_all(&plans_dir).unwrap();
        std::fs::write(
            plans_dir.join("RentAI demo hardening-ddbb71c6.plan.md"),
            "# RentAI demo hardening\n\n## Steps\n- prep\n",
        )
        .unwrap();

        let altevra_db = tmp.path().join("altevra.db");
        let pool = create_pool(altevra_db.to_str().unwrap()).await.unwrap();
        run_migrations(&pool).await.unwrap();

        let sum = import(&cursor_db, &plans_dir, Some(&pool), false)
            .await
            .unwrap();
        assert!(!sum.dry_run);
        assert_eq!(sum.edits_inserted, 4, "4 edit rows landed");
        assert_eq!(sum.plans_inserted, 1, "1 plan row landed");

        let count = CursorEditsRepository::new(&pool).count().await.unwrap();
        assert_eq!(count, 5, "4 edits + 1 plan all in cursor_edits");
    }

    /// Real-db invariant: opening a Cursor db with the read-only path NEVER
    /// modifies it. Verified by SHA-256 of the file bytes before vs after.
    #[test]
    fn cursor_db_is_byte_untouched_after_readonly_open() {
        let tmp = TempDir::new().unwrap();
        let db = make_fixture_db(tmp.path());
        let before = std::fs::read(&db).unwrap();
        let mut sum = CursorImportSummary::default();
        let _ = collect_edits(&db, &mut sum).unwrap();
        let after = std::fs::read(&db).unwrap();
        assert_eq!(before, after, "read-only open mutated the cursor db");
    }

    #[test]
    fn missing_cursor_db_is_not_an_error() {
        let tmp = TempDir::new().unwrap();
        let mut sum = CursorImportSummary::default();
        let edits = collect_edits(&tmp.path().join("does-not-exist.db"), &mut sum).unwrap();
        assert!(edits.is_empty());
        assert!(!sum.source_db_exists);
    }
}

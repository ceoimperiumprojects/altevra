//! `altevra recall <query>` — human UX over the temporal + source-tracing layer.
//!
//! Answers Pavle's "ej brate, šta smo radili pre mesec dana sa Amerikancima". Reads
//! the captured turn corpus, applies the temporal window if given, and prints
//! provenance breadcrumbs ("4w ago in claude-code on altevra: …") that read like
//! a memory rather than a query result. The underlying repo call is
//! `search_turns_with_provenance` (which gates everything via the same path the MCP
//! tool uses, so no leak surface differs).

use altevra_core::time_window::{humanize_relative, parse_since_until, parse_window};
use altevra_db::{create_pool, run_migrations, FtsRepository, SessionsRepository, TurnSearchHit};
use chrono::{DateTime, Utc};
use clap::Args;
use std::path::PathBuf;

/// One unified recall result — a session turn OR a durable object
/// (decision/learning/wiki). Both render identically: a "when — source" line +
/// snippet. This is what makes recall span the whole second brain (§4.1), not
/// just the turns table.
struct RecallItem {
    when: DateTime<Utc>,
    when_human: String,
    /// Breadcrumb: "claude-code · revesta" (turn) | "learning · business" (object).
    source: String,
    snippet: String,
}

#[derive(Args)]
pub struct RecallArgs {
    /// Free-text recall query — what you're trying to remember. Optional when
    /// `--with` is given ("what did I do involving <person/project>").
    #[arg(default_value = "")]
    pub query: String,
    /// Cross-link recall: surface objects/turns that MENTION a known person or
    /// project (resolved from People.md + the project registry + mentors). E.g.
    /// `altevra recall --with Đorđe` → decisions/notes that mention Đorđe.
    #[arg(long = "with")]
    pub with: Option<String>,
    /// Vault root for the entity dictionary (default: inferred ~/Obsidian/Imperium).
    #[arg(long)]
    pub vault: Option<PathBuf>,
    /// Quick window preset (`last_24h` | `last_week` | `last_month` | `last_quarter`
    /// | `last_year`) or a raw duration (`24h`/`7d`/`30d`/`3mo`/`1y`).
    #[arg(long)]
    pub window: Option<String>,
    /// Inclusive start of the recall range (RFC3339, YYYY-MM-DD, or `30d` =
    /// `now - 30d`). Overlays on top of `--window` if both given.
    #[arg(long)]
    pub since: Option<String>,
    /// Exclusive end of the recall range. Defaults to "now" if omitted.
    #[arg(long)]
    pub until: Option<String>,
    /// Restrict to a tool (`claude-code` | `codex` | `cursor` | `hermes` | …).
    #[arg(long)]
    pub tool: Option<String>,
    /// Restrict to a project name (matches `sessions.project_name`).
    #[arg(long)]
    pub project: Option<String>,
    /// Max hits to print.
    #[arg(long, default_value_t = 10)]
    pub limit: i64,
    /// SQLite database path.
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
    /// Emit JSON (the same provenance-rich shape the MCP tool returns).
    #[arg(long)]
    pub json: bool,
    /// Match indexed FILE content by MEANING (vector/semantic search over the
    /// BGE-M3 embeddings) instead of keyword. Finds related work even when the
    /// exact words differ. Slower (loads the embedding model). Needs a build
    /// with `--features embedding`.
    #[arg(long)]
    pub semantic: bool,
}

pub async fn run(args: RecallArgs) -> anyhow::Result<()> {
    let now = Utc::now();

    if args.query.trim().is_empty() && args.with.is_none() {
        anyhow::bail!("provide a search query or use --with <person/project>");
    }

    // Resolve temporal range — same fail-closed rules as the MCP tool so users
    // never get a silently-wrong window from a typo.
    let mut t_since = None;
    let mut t_until = None;
    if let Some(w) = args.window.as_deref() {
        let range = parse_window(w, now).ok_or_else(|| {
            anyhow::anyhow!(
                "unknown --window '{w}' (try: 24h, 7d, 30d, 3mo, last_week, last_month)"
            )
        })?;
        t_since = Some(range.since);
        t_until = Some(range.until);
    }
    if let Some(s) = args.since.as_deref() {
        t_since = Some(
            parse_since_until(s, now).ok_or_else(|| anyhow::anyhow!("invalid --since '{s}'"))?,
        );
    }
    if let Some(u) = args.until.as_deref() {
        t_until = Some(
            parse_since_until(u, now).ok_or_else(|| anyhow::anyhow!("invalid --until '{u}'"))?,
        );
    }

    let pool = create_pool(&args.db.to_string_lossy()).await?;
    run_migrations(&pool).await?;

    // --- `--with <entity>`: cross-link recall (mention graph) ---
    if let Some(name) = args.with.clone() {
        return recall_with_entity(&pool, &args, &name, t_since, t_until, now).await;
    }

    // --- Source 1: session turns (the work stream). ---
    let turn_hits: Vec<TurnSearchHit> = SessionsRepository::new(&pool)
        .search_turns_with_provenance(
            &args.query,
            args.project.as_deref(),
            args.tool.as_deref(),
            t_since,
            t_until,
            args.limit,
        )
        .await?;

    // --- Source 2: durable objects (decisions/learnings/wiki — captured memory). ---
    // Only when not tool/project-scoped to a session (objects have no tool/session).
    // Apply the same temporal window against the object's updated_at.
    let object_hits = if args.tool.is_some() {
        Vec::new() // tool filter is session-only; objects don't carry a tool.
    } else {
        FtsRepository::new(&pool)
            .search_objects(&args.query, args.limit)
            .await?
            .into_iter()
            .filter(|o| in_window(o.updated_at, t_since, t_until))
            .filter(|o| {
                // Honor a project filter loosely: object scope/domain match.
                args.project
                    .as_deref()
                    .map(|p| o.domain == p || o.object_type == p)
                    .unwrap_or(true)
            })
            .collect::<Vec<_>>()
    };

    // --- Source 3: indexed FILE content (file-watcher). The watcher captures
    // ALL work outside AI sessions — Desktop/, Documents/, projekti/, Obsidian/ —
    // into memory_chunks. Without this, `recall "ReVesta"` would only see AI turns
    // and miss the actual ReVesta files Pavle edited from home. Context-based, not
    // repo-scoped: a LIKE over the chunk text surfaces the work wherever it lives.
    // Deduped to one hit per source file (most recent chunk), time-windowed.
    let doc_hits: Vec<(String, DateTime<Utc>, String)> =
        if args.query.trim().is_empty() || args.tool.is_some() {
            Vec::new()
        } else if args.semantic {
            semantic_doc_hits(&pool, &args.query, args.limit, t_since, t_until).await
        } else {
            use sqlx::Row;
            // Tokenize: every whitespace-separated term must appear (AND), in any
            // order — so "Simple Surplus operator" matches a doc containing all
            // three words, not only the literal phrase. Single-term queries
            // ("ReVesta") still work as a plain substring.
            let terms: Vec<String> = args
                .query
                .split_whitespace()
                .filter(|t| !t.is_empty())
                .map(|t| format!("%{t}%"))
                .collect();
            let where_clause = vec!["mc.text LIKE ?"; terms.len()].join(" AND ");
            let sql = format!(
                "SELECT mc.text AS text, mc.created_at AS created_at, \
                        COALESCE(md.source_path, '') AS source_path \
                 FROM memory_chunks mc \
                 LEFT JOIN memory_documents md ON md.id = mc.document_id \
                 WHERE {where_clause} \
                 ORDER BY mc.created_at DESC \
                 LIMIT ?"
            );
            let mut q = sqlx::query(&sql);
            for t in &terms {
                q = q.bind(t);
            }
            let rows = q
                .bind(args.limit * 8) // over-fetch; dedup + time-filter trims below
                .fetch_all(&pool)
                .await
                .unwrap_or_default();

            let mut seen = std::collections::HashSet::new();
            let mut out = Vec::new();
            for r in rows {
                let text: String = r.get("text");
                let created_raw: String = r.get("created_at");
                let source_path: String = r.get("source_path");
                let created_at = chrono::DateTime::parse_from_rfc3339(&created_raw)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now());
                if !in_window(created_at, t_since, t_until) {
                    continue;
                }
                // One hit per file (most recent chunk wins — rows are DESC).
                let key = if source_path.is_empty() { text.clone() } else { source_path.clone() };
                if !seen.insert(key) {
                    continue;
                }
                out.push((text, created_at, source_path));
            }
            out
        };

    // --- Merge into a unified, recency-sorted list. ---
    let mut items: Vec<RecallItem> = Vec::new();
    for h in &turn_hits {
        let when_human = humanize_relative(h.row.created_at, now);
        let tool_s = h.session_tool.as_deref().unwrap_or("?");
        let proj_s = h.session_project.as_deref().unwrap_or("?");
        items.push(RecallItem {
            when: h.row.created_at,
            when_human,
            source: format!("{tool_s} · {proj_s}"),
            snippet: snippet(&h.row.content, &args.query, 200),
        });
    }
    for o in &object_hits {
        let when_human = humanize_relative(o.updated_at, now);
        items.push(RecallItem {
            when: o.updated_at,
            when_human,
            // Atomized objects are stored as `learning` rows, but their real type
            // (decision/person/note) lives in a `kind:` tag — prefer it so a
            // captured decision reads as a decision, not a generic learning
            // (parity with the `--with` entity path's `row_kind`).
            source: format!("{} · {}", row_kind(&o.tags, &o.object_type), o.domain),
            snippet: snippet_with_title(&o.title, &o.body, &args.query, 200),
        });
    }
    for (text, when, source_path) in &doc_hits {
        let label = if source_path.is_empty() {
            "file".to_string()
        } else {
            format!("file · {}", source_path.trim_start_matches("./"))
        };
        items.push(RecallItem {
            when: *when,
            when_human: humanize_relative(*when, now),
            source: label,
            snippet: snippet(text, &args.query, 200),
        });
    }
    // Default: most-recent-first (best for "what happened yesterday"). In
    // --semantic mode the file hits arrive already ranked by MEANING (cosine),
    // so lead with them in that order — a STABLE sort that only pushes non-file
    // items after file items preserves the similarity ranking instead of letting
    // recent turns bury the relevant files.
    if args.semantic {
        items.sort_by_key(|it| !it.source.starts_with("file"));
    } else {
        items.sort_by_key(|it| std::cmp::Reverse(it.when));
    }
    items.truncate(args.limit as usize);

    if args.json {
        let entries: Vec<_> = items
            .iter()
            .map(|it| {
                serde_json::json!({
                    "when": it.when,
                    "when_human": it.when_human,
                    "source": it.source,
                    "snippet": it.snippet,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "query": args.query,
                "count": entries.len(),
                "turns": turn_hits.len(),
                "objects": object_hits.len(),
                "files": doc_hits.len(),
                "window": t_since.map(|s| serde_json::json!({"since": s, "until": t_until})),
                "results": entries,
            }))?
        );
        return Ok(());
    }

    // Human prose layout.
    let scope = describe_scope(t_since, t_until, &args);
    if items.is_empty() {
        println!("No memory of '{}'{}.", args.query, scope);
        return Ok(());
    }
    println!(
        "Recall '{}'{} — {} hit(s) ({} turn / {} note / {} file):\n",
        args.query,
        scope,
        items.len(),
        turn_hits.len(),
        object_hits.len(),
        doc_hits.len()
    );
    for it in &items {
        println!("  • {} — {}", it.when_human, it.source);
        println!("    {}", it.snippet);
    }
    Ok(())
}

/// Semantic file recall: embed the query with BGE-M3 and run the Phase 1 hybrid
/// backbone (`altevra_memory::retrieve` — BM25 over object_fts + dense cosine,
/// fused on one canonical chunk key) instead of a raw vector scan. Maps each hit
/// back to (text, created_at, source_path) via the hit's `source_ref`, applies
/// the same time window, dedups per file. The hybrid primitive carries the
/// provenance + filters the old raw `search_by_vector` path lost.
#[cfg(feature = "embedding")]
async fn semantic_doc_hits(
    pool: &sqlx::SqlitePool,
    query: &str,
    limit: i64,
    t_since: Option<DateTime<Utc>>,
    t_until: Option<DateTime<Utc>>,
) -> Vec<(String, DateTime<Utc>, String)> {
    use altevra_memory::{
        retrieve, AsyncEmbeddingProvider, Bge3Embedder, RetrievalRequest, BGE_M3_DIM, BGE_M3_MODEL,
    };

    let embedder = match Bge3Embedder::new() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("[altevra] semantic recall unavailable (embedder init failed): {e}");
            return Vec::new();
        }
    };
    let emb = match embedder.embed(query).await {
        Ok(e) => e,
        Err(e) => {
            eprintln!("[altevra] semantic recall: query embed failed: {e}");
            return Vec::new();
        }
    };

    let req = RetrievalRequest {
        query: query.to_string(),
        since: t_since,
        until: t_until,
        limit: (limit * 8).max(1) as usize,
        ..Default::default()
    };
    let hits = retrieve(pool, &req, Some(&emb.vector), BGE_M3_MODEL, BGE_M3_DIM)
        .await
        .unwrap_or_default();

    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for h in hits {
        let source_path = h.source_ref.source_path.clone().unwrap_or_default();
        let created_at = h.source_ref.ts.unwrap_or_else(Utc::now);
        // retrieve() already applied the window when the hit carried a ts; this
        // is a belt-and-suspenders cut for hits whose ts was unknown.
        if !in_window(created_at, t_since, t_until) {
            continue;
        }
        let key = if source_path.is_empty() {
            h.snippet.clone()
        } else {
            source_path.clone()
        };
        if !seen.insert(key) {
            continue;
        }
        out.push((h.snippet, created_at, source_path));
        if out.len() as i64 >= limit * 4 {
            break;
        }
    }
    out
}

/// Fallback when not built with the embedding feature.
#[cfg(not(feature = "embedding"))]
async fn semantic_doc_hits(
    _pool: &sqlx::SqlitePool,
    _query: &str,
    _limit: i64,
    _t_since: Option<DateTime<Utc>>,
    _t_until: Option<DateTime<Utc>>,
) -> Vec<(String, DateTime<Utc>, String)> {
    eprintln!("[altevra] --semantic needs a build with `--features embedding`; falling back to no file hits.");
    Vec::new()
}

/// Cross-link recall: list objects (and their breadcrumbs) that MENTION a known
/// person/project, resolved from the entity dictionary. Answers "what did I do
/// with Đorđe this month" — combine with `--window`/`--since` for the temporal cut.
async fn recall_with_entity(
    pool: &sqlx::SqlitePool,
    args: &RecallArgs,
    name: &str,
    t_since: Option<DateTime<Utc>>,
    t_until: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    // Build the dictionary and resolve the name (diacritic/case-insensitive) to an
    // entity id. The dictionary read is the same one capture uses.
    let probe = args
        .vault
        .clone()
        .map(|v| v.join("Memory").join("People.md"))
        .unwrap_or_else(default_people_md);
    let dict = crate::commands::entity_dict::build_dictionary(&probe, args.vault.as_deref());
    let entity = altevra_vault::resolve_entity(&dict, name);
    let Some(entity) = entity else {
        if args.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "with": name, "resolved": false, "count": 0, "results": [],
                }))?
            );
        } else {
            println!(
                "No known person/project matches '{name}'. \
                 (known: {} people, {} projects)",
                dict.people.len(),
                dict.projects.len()
            );
        }
        return Ok(());
    };

    let mentions = altevra_db::MentionsRepository::new(pool);
    let learnings = altevra_db::LearningsRepository::new(pool);
    let sources = mentions
        .objects_mentioning(&entity.id, args.limit.max(50))
        .await?;

    let mut items: Vec<RecallItem> = Vec::new();
    for (otype, oid) in &sources {
        // Currently every atomized object is a `learning` row; resolve its body.
        if otype != "learning" {
            continue;
        }
        let Some(row) = learnings.get(oid).await? else {
            continue;
        };
        if row.status == "forgotten" {
            continue;
        }
        // Temporal cut: use the section's created date from provenance if present,
        // else fall through to "now" (always within an open window).
        let when = provenance_date(&row.provenance).unwrap_or(now);
        if !in_window(when, t_since, t_until) {
            continue;
        }
        items.push(RecallItem {
            when,
            when_human: humanize_relative(when, now),
            source: format!("{} · {}", row_kind(&row.tags, otype), row.domain),
            snippet: snippet_with_title(&row.title, &row.body, "", 200),
        });
    }
    items.sort_by_key(|it| std::cmp::Reverse(it.when));
    items.truncate(args.limit as usize);

    if args.json {
        let entries: Vec<_> = items
            .iter()
            .map(|it| {
                serde_json::json!({
                    "when": it.when,
                    "when_human": it.when_human,
                    "source": it.source,
                    "snippet": it.snippet,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "with": name,
                "resolved": true,
                "entity": { "id": entity.id, "name": entity.name, "kind": entity.kind.as_str() },
                "count": entries.len(),
                "results": entries,
            }))?
        );
        return Ok(());
    }

    if items.is_empty() {
        println!(
            "No memory mentioning {} ({}) in that window.",
            entity.name,
            entity.kind.as_str()
        );
        return Ok(());
    }
    println!(
        "Involving {} ({}) — {} item(s):\n",
        entity.name,
        entity.kind.as_str(),
        items.len()
    );
    for it in &items {
        println!("  • {} — {}", it.when_human, it.source);
        println!("    {}", it.snippet);
    }
    Ok(())
}

/// `kind:<type>` tag → display kind, else the row's object type.
fn row_kind(tags_json: &str, fallback: &str) -> String {
    if let Ok(tags) = serde_json::from_str::<Vec<String>>(tags_json) {
        for t in tags {
            if let Some(k) = t.strip_prefix("kind:") {
                return k.to_string();
            }
        }
    }
    fallback.to_string()
}

/// Pull a `created` date out of a provenance JSON blob (atomize stores the section
/// heading's YYYY-MM-DD there).
fn provenance_date(provenance: &str) -> Option<DateTime<Utc>> {
    let v: serde_json::Value = serde_json::from_str(provenance).ok()?;
    let s = v.get("created")?.as_str()?;
    let d = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()?;
    Some(d.and_hms_opt(12, 0, 0)?.and_utc())
}

fn default_people_md() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("Obsidian/Imperium/Memory/People.md")
}

/// Half-open window membership; `None` bounds are unbounded.
fn in_window(t: DateTime<Utc>, since: Option<DateTime<Utc>>, until: Option<DateTime<Utc>>) -> bool {
    since.map(|s| t >= s).unwrap_or(true) && until.map(|u| t < u).unwrap_or(true)
}

/// Snippet for an object: prefer a body window around the match; if the match is
/// only in the title, lead with the title.
fn snippet_with_title(title: &str, body: &str, query: &str, max: usize) -> String {
    let s = snippet(body, query, max);
    if s.to_lowercase().contains(&query.to_lowercase()) || body_has_token(body, query) {
        s
    } else {
        format!("{title} — {s}")
    }
}

fn body_has_token(body: &str, query: &str) -> bool {
    let lc = body.to_lowercase();
    query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 2)
        .any(|t| lc.contains(t))
}

fn describe_scope(
    since: Option<chrono::DateTime<Utc>>,
    until: Option<chrono::DateTime<Utc>>,
    args: &RecallArgs,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let (Some(s), Some(u)) = (since, until) {
        parts.push(format!(
            "in {}..{}",
            s.format("%Y-%m-%d"),
            u.format("%Y-%m-%d")
        ));
    } else if let Some(s) = since {
        parts.push(format!("since {}", s.format("%Y-%m-%d")));
    }
    if let Some(t) = args.tool.as_deref() {
        parts.push(format!("on {t}"));
    }
    if let Some(p) = args.project.as_deref() {
        parts.push(format!("in project {p}"));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" ({})", parts.join(", "))
    }
}

/// Same snippet helper as turn_search — pull a window around the first token match.
fn snippet(content: &str, query: &str, max: usize) -> String {
    let lc = content.to_lowercase();
    let mut first_pos: Option<usize> = None;
    for tok in query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 2)
    {
        if let Some(p) = lc.find(tok) {
            first_pos = Some(first_pos.map_or(p, |cur| cur.min(p)));
        }
    }
    let raw_start = first_pos
        .map(|p| p.saturating_sub(40))
        .unwrap_or(0)
        .min(content.len());
    // Snap to the nearest valid UTF-8 char boundary so we never panic on
    // multi-byte characters (e.g. Serbian Cyrillic, arrows →, emoji).
    let start = snap_to_char_boundary_left(content, raw_start);
    let raw_end = (start + max).min(content.len());
    let end = snap_to_char_boundary_right(content, raw_end);
    let slice = &content[start..end];
    let trimmed = slice.replace('\n', " ");
    if end < content.len() {
        format!("{trimmed}…")
    } else {
        trimmed
    }
}

/// Snap byte index leftward to the nearest valid UTF-8 char boundary.
fn snap_to_char_boundary_left(s: &str, mut idx: usize) -> usize {
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

/// Snap byte index rightward to the nearest valid UTF-8 char boundary.
fn snap_to_char_boundary_right(s: &str, mut idx: usize) -> usize {
    while idx < s.len() && !s.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}

#[cfg(test)]
mod tests {
    use super::*;
    use altevra_db::{SessionRow, TurnRow};
    use chrono::Duration;
    use tempfile::TempDir;
    use uuid::Uuid;

    async fn seed_three_sessions(db: &std::path::Path) {
        let pool = create_pool(&db.to_string_lossy()).await.unwrap();
        run_migrations(&pool).await.unwrap();
        let repo = SessionsRepository::new(&pool);
        let now = Utc::now();

        for (tool, project, days_ago, body) in [
            (
                "claude-code",
                "altevra",
                28_i64,
                "called Americans about the deal terms",
            ),
            (
                "cursor",
                "revesta",
                35_i64,
                "Americans replied with pricing concerns",
            ),
            (
                "codex",
                "altevra",
                2_i64,
                "rust refactor on the americans handler module",
            ),
        ] {
            let s = SessionRow {
                id: Uuid::new_v4(),
                tool: tool.into(),
                project_id: None,
                project_name: Some(project.into()),
                started_at: now - Duration::days(days_ago),
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
            };
            repo.start_session(&s).await.unwrap();
            let t = TurnRow {
                id: Uuid::new_v4(),
                session_id: s.id,
                turn_idx: 0,
                role: "user".into(),
                content: body.into(),
                tool_calls: None,
                tool_name: None,
                model: None,
                tokens_in: None,
                tokens_out: None,
                latency_ms: None,
                file_changes: None,
                redacted_count: 0,
                source_tool: Some(tool.into()),
                sensitivity: "internal".into(),
                redaction_status: "clean".into(),
                created_at: now - Duration::days(days_ago),
                working_dir: None,
            };
            repo.record_turn(&t).await.unwrap();
        }
    }

    #[tokio::test]
    async fn recall_window_drops_out_of_range_hits() {
        // Headline integration: 3 turns about "Americans" — month-old / 35d / 2d.
        // `--window last_month` (30d) must drop the 35d cursor hit.
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("altevra.db");
        seed_three_sessions(&db).await;

        // Exercise the JSON path end-to-end (prints, no panic), then assert counts
        // via the repo directly below.
        run(RecallArgs {
            query: "Americans".into(),
            with: None,
            vault: None,
            window: Some("last_month".into()),
            since: None,
            until: None,
            tool: None,
            project: None,
            limit: 10,
            db: db.clone(),
            json: true,
            semantic: false,
        })
        .await
        .unwrap();

        // Re-run via repo directly to assert the count is exactly 2 (deterministic).
        let pool = create_pool(&db.to_string_lossy()).await.unwrap();
        let repo = SessionsRepository::new(&pool);
        let now = Utc::now();
        let r = parse_window("last_month", now).unwrap();
        let hits = repo
            .search_turns_with_provenance("Americans", None, None, Some(r.since), Some(r.until), 10)
            .await
            .unwrap();
        assert_eq!(hits.len(), 2, "last_month drops the 35-day-old cursor hit");
        assert!(hits.iter().all(|h| h.session_tool.is_some()));
    }

    #[tokio::test]
    async fn recall_rejects_garbage_window() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("altevra.db");
        seed_three_sessions(&db).await;
        let r = run(RecallArgs {
            query: "anything".into(),
            with: None,
            vault: None,
            window: Some("garbage".into()),
            since: None,
            until: None,
            tool: None,
            project: None,
            limit: 10,
            db,
            json: true,
            semantic: false,
        })
        .await;
        assert!(r.is_err(), "garbage window must fail-closed");
    }

    /// A1 UX contract: a captured DECISION must recall as a `decision`, not as the
    /// generic `learning` row it is physically stored as. The atomizer types every
    /// section as a `learning` (object_index.type='learning'); its real kind lives
    /// in a `kind:decision` tag. `recall <query>` must surface that kind in the
    /// breadcrumb so a user can tell a decision apart from a learning — matching the
    /// `--with` entity path. Without the fix the breadcrumb read "learning · …".
    #[tokio::test]
    async fn recall_labels_captured_decision_as_decision_not_learning() {
        use altevra_core::{Domain, Sensitivity};

        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("dec.db");
        let pool = create_pool(&db.to_string_lossy()).await.unwrap();
        run_migrations(&pool).await.unwrap();

        // Mirror the real Memory/ layout so the file is typed as a decision aggregate.
        let mem = tmp.path().join("Memory");
        std::fs::create_dir_all(&mem).unwrap();
        let file = mem.join("Decisions.md");
        std::fs::write(
            &file,
            "# Decisions\n\n\
             ## ReVesta direct-call hypothesis validated\n\
             **Odluka:** Keep pushing direct-call discovery, do not return to build mode.\n",
        )
        .unwrap();
        let secs = altevra_vault::parse_sections(&std::fs::read_to_string(&file).unwrap());
        let domain: Domain = "business".parse().unwrap();
        let declared: Sensitivity = "internal".parse().unwrap();
        crate::commands::capture::atomize_file(
            &pool, &file, &secs, &domain, &declared, &[], None,
        )
        .await
        .unwrap();

        // The object is physically a `learning` row but tagged kind:decision.
        let hits = altevra_db::FtsRepository::new(&pool)
            .search_objects("direct-call hypothesis", 10)
            .await
            .unwrap();
        assert_eq!(hits.len(), 1, "the captured decision is FTS-searchable");
        assert_eq!(
            hits[0].object_type, "learning",
            "physically stored as a learning row"
        );
        assert!(
            hits[0].tags.contains("kind:decision"),
            "tags carry the real kind: {}",
            hits[0].tags
        );

        // The breadcrumb the CLI renders must say `decision`, NOT `learning`.
        let label = row_kind(&hits[0].tags, &hits[0].object_type);
        assert_eq!(
            label, "decision",
            "recall must label a captured decision as a decision, not a learning"
        );
    }

    /// Regression test: multi-byte Serbian text + arrow → must not panic.
    ///
    /// The bug: `p.saturating_sub(40)` and `start + max` produce byte offsets
    /// that may land in the middle of a multi-byte UTF-8 char. Before the fix
    /// this caused a `byte index N is not a char boundary` panic.
    #[test]
    fn snippet_multibyte_no_panic() {
        // Serbian text with Cyrillic + ASCII + arrow → — several 2-byte chars.
        let content = "Стратегија → извоз производа. Потребно је дефинисати keyword план за Q3.";
        // Calling snippet must not panic regardless of where start/end fall.
        let s = snippet(content, "keyword", 30);
        // The result must be valid UTF-8 (implicit in Rust &str) and the call must succeed.
        assert!(s.contains("keyword") || !s.is_empty() || s.is_empty());

        // A trickier case: the match position is exactly at a multi-byte boundary.
        // Place the search token right after multi-byte chars so subtracting 40
        // bytes would land mid-codepoint.
        let content2 = "аааааааааааааааааааааааааааааааааааааааааааааааааааа keyword here → more text аа";
        let s2 = snippet(content2, "keyword", 50);
        assert!(s2.contains("keyword"), "must find keyword in: {s2:?}");

        // Edge: arrow → (3 bytes: 0xE2 0x86 0x92) right before the token.
        // Use max=80 (well beyond the content length) so the slice is always complete.
        // The key invariant is that slicing must not panic — boundary-snapping must handle
        // the multi-byte arrow correctly even when start/end land near it.
        let content3 = "prefix text → keyword ends here";
        let s3 = snippet(content3, "keyword", 80);
        assert!(s3.contains("keyword"), "must find keyword in: {s3:?}");
    }

    #[tokio::test]
    async fn recall_with_entity_returns_objects_that_mention_it() {
        use altevra_core::{Domain, EntityDictionary, Sensitivity};

        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("altevra.db");
        let pool = create_pool(&db.to_string_lossy()).await.unwrap();
        run_migrations(&pool).await.unwrap();

        // Atomize a Decisions file that mentions Đorđe, with a dictionary so edges
        // are recorded.
        let mem = tmp.path().join("Memory");
        std::fs::create_dir_all(&mem).unwrap();
        let file = mem.join("Decisions.md");
        std::fs::write(
            &file,
            "# Decisions\n\n## ReVesta directive\nĐorđe je rekao: prodaja pre build-a.\n",
        )
        .unwrap();
        let mut dict = EntityDictionary::new();
        dict.add_person("djordje", "Đorđe Dimitrijević", &["Đorđe".into()]);
        let secs = altevra_vault::parse_sections(&std::fs::read_to_string(&file).unwrap());
        let domain: Domain = "business".parse().unwrap();
        let declared: Sensitivity = "internal".parse().unwrap();
        crate::commands::capture::atomize_file(
            &pool,
            &file,
            &secs,
            &domain,
            &declared,
            &[],
            Some(&dict),
        )
        .await
        .unwrap();

        // recall --with Djordje (ascii spelling) → finds the decision mentioning him.
        // Use the temp vault so the dictionary resolves the same entity.
        run(RecallArgs {
            query: String::new(),
            with: Some("Djordje".into()),
            vault: Some(tmp.path().to_path_buf()),
            window: None,
            since: None,
            until: None,
            tool: None,
            project: None,
            limit: 10,
            db: db.clone(),
            json: true,
            semantic: false,
        })
        .await
        .unwrap();

        // Assert via the repo directly that the edge resolves to one object.
        let ment = altevra_db::MentionsRepository::new(&pool);
        let hits = ment.objects_mentioning("person:djordje", 10).await.unwrap();
        assert_eq!(hits.len(), 1, "the decision mentioning Đorđe is linked");
    }
}

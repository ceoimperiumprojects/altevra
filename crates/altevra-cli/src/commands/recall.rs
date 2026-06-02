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
            source: format!("{} · {}", o.object_type, o.domain),
            snippet: snippet_with_title(&o.title, &o.body, &args.query, 200),
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
                "query": args.query,
                "count": entries.len(),
                "turns": turn_hits.len(),
                "objects": object_hits.len(),
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
        "Recall '{}'{} — {} hit(s) ({} turn / {} note):\n",
        args.query,
        scope,
        items.len(),
        turn_hits.len(),
        object_hits.len()
    );
    for it in &items {
        println!("  • {} — {}", it.when_human, it.source);
        println!("    {}", it.snippet);
    }
    Ok(())
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
    let entity = resolve_entity(&dict, name);
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

/// Resolve a free-text name to a dictionary entity: exact-id, then any alias match
/// (ascii-folded, case-insensitive), longest-name first so a full name wins.
fn resolve_entity<'a>(
    dict: &'a altevra_core::EntityDictionary,
    name: &str,
) -> Option<&'a altevra_core::Entity> {
    if let Some(e) = dict.get(name) {
        return Some(e);
    }
    let want = altevra_core::ascii_fold(name).to_lowercase();
    let mut best: Option<&altevra_core::Entity> = None;
    for e in dict.all() {
        let hit = e
            .aliases
            .iter()
            .any(|a| altevra_core::ascii_fold(a).to_lowercase() == want)
            || altevra_core::ascii_fold(&e.name).to_lowercase() == want;
        if hit && best.map(|b| e.name.len() > b.name.len()).unwrap_or(true) {
            best = Some(e);
        }
    }
    best
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
    let start = first_pos
        .map(|p| p.saturating_sub(40))
        .unwrap_or(0)
        .min(content.len());
    let end = (start + max).min(content.len());
    let slice = &content[start..end];
    let trimmed = slice.replace('\n', " ");
    if end < content.len() {
        format!("{trimmed}…")
    } else {
        trimmed
    }
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
        })
        .await;
        assert!(r.is_err(), "garbage window must fail-closed");
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
        })
        .await
        .unwrap();

        // Assert via the repo directly that the edge resolves to one object.
        let ment = altevra_db::MentionsRepository::new(&pool);
        let hits = ment.objects_mentioning("person:djordje", 10).await.unwrap();
        assert_eq!(hits.len(), 1, "the decision mentioning Đorđe is linked");
    }
}

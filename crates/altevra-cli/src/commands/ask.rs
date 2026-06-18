//! `altevra ask "<question>"` — the smarter finder. Retrieves across ALL Altevra
//! sources (session turns + durable objects + indexed file content), then a cheap
//! fast model (Haiku via `claude -p`) READS the retrieved context and answers the
//! question precisely, with sources. Recall gives raw hits; `ask` gives an answer.

use altevra_core::time_window::{parse_since_until, parse_window};
use altevra_db::{create_pool, run_migrations, FtsRepository, SessionsRepository};
use altevra_llm::{ChatMessage, ChatOpts, ChatProvider, ClaudeCliProvider};
use chrono::{DateTime, Utc};
use clap::Args;
use std::path::PathBuf;

#[derive(Args)]
pub struct AskArgs {
    /// The question to answer from your second brain.
    pub question: String,
    /// How many context snippets to feed the model.
    #[arg(long, default_value_t = 12)]
    pub limit: i64,
    /// Quick window preset (last_24h|last_week|last_month|…) or duration (24h/7d/30d).
    #[arg(long)]
    pub window: Option<String>,
    /// Inclusive start (RFC3339, YYYY-MM-DD, or relative 30d). Overlays --window.
    #[arg(long)]
    pub since: Option<String>,
    /// Exclusive end (default: now).
    #[arg(long)]
    pub until: Option<String>,
    /// Model for the answer (cheap + fast by default).
    #[arg(long, default_value = "claude-haiku-4-5-20251001")]
    pub model: String,
    /// SQLite database path.
    #[arg(long, default_value_os_t = altevra_core::default_db_path())]
    pub db: PathBuf,
}

pub async fn run(args: AskArgs) -> anyhow::Result<()> {
    let now = Utc::now();
    if args.question.trim().is_empty() {
        anyhow::bail!("ask what? provide a question");
    }

    // --- time window (same rules as recall/MCP) ---
    let mut t_since: Option<DateTime<Utc>> = None;
    let mut t_until: Option<DateTime<Utc>> = None;
    if let Some(w) = args.window.as_deref() {
        if let Some(r) = parse_window(w, now) {
            t_since = Some(r.since);
            t_until = Some(r.until);
        }
    }
    if let Some(s) = args.since.as_deref() {
        t_since = parse_since_until(s, now);
    }
    if let Some(u) = args.until.as_deref() {
        t_until = parse_since_until(u, now);
    }

    let pool = create_pool(&args.db.to_string_lossy()).await?;
    run_migrations(&pool).await?;

    // --- RETRIEVE across all sources ---
    let mut ctx_lines: Vec<String> = Vec::new();
    let mut sources: Vec<String> = Vec::new();

    // 1. session turns
    if let Ok(hits) = SessionsRepository::new(&pool)
        .search_turns_with_provenance(&args.question, None, None, t_since, t_until, args.limit)
        .await
    {
        for h in hits {
            let when = altevra_core::time_window::humanize_relative(h.row.created_at, now);
            let tool = h.session_tool.as_deref().unwrap_or("?");
            let snip = h.row.content.chars().take(400).collect::<String>();
            ctx_lines.push(format!("[turn · {tool} · {when}] {snip}"));
        }
    }

    // 2. durable objects (decisions/learnings/wiki/insights)
    if let Ok(objs) = FtsRepository::new(&pool).search_objects(&args.question, args.limit).await {
        for o in objs.into_iter().filter(|o| in_window(o.updated_at, t_since, t_until)) {
            let when = altevra_core::time_window::humanize_relative(o.updated_at, now);
            let snip = o.body.chars().take(400).collect::<String>();
            ctx_lines.push(format!("[{} · {when}] {}: {snip}", o.object_type, o.title));
        }
    }

    // 3. indexed file content (tokenized LIKE — every term must appear)
    {
        use sqlx::Row;
        let terms: Vec<String> = args
            .question
            .split_whitespace()
            .filter(|t| t.len() > 2)
            .take(6)
            .map(|t| format!("%{t}%"))
            .collect();
        if !terms.is_empty() {
            let where_clause = vec!["mc.text LIKE ?"; terms.len()].join(" AND ");
            let sql = format!(
                "SELECT mc.text AS text, mc.created_at AS created_at, \
                        COALESCE(md.source_path,'') AS source_path \
                 FROM memory_chunks mc LEFT JOIN memory_documents md ON md.id = mc.document_id \
                 WHERE {where_clause} ORDER BY mc.created_at DESC LIMIT ?"
            );
            let mut q = sqlx::query(&sql);
            for t in &terms {
                q = q.bind(t);
            }
            if let Ok(rows) = q.bind(args.limit).fetch_all(&pool).await {
                let mut seen = std::collections::HashSet::new();
                for r in rows {
                    let text: String = r.get("text");
                    let created: String = r.get("created_at");
                    let path: String = r.get("source_path");
                    let when = chrono::DateTime::parse_from_rfc3339(&created)
                        .map(|d| d.with_timezone(&Utc))
                        .unwrap_or(now);
                    if !in_window(when, t_since, t_until) {
                        continue;
                    }
                    if !seen.insert(path.clone()) {
                        continue;
                    }
                    let label = path.trim_start_matches("./");
                    let snip = text.chars().take(400).collect::<String>();
                    ctx_lines.push(format!("[file · {label}] {snip}"));
                    if !label.is_empty() {
                        sources.push(label.to_string());
                    }
                }
            }
        }
    }

    // 4. SEMANTIC file content — embed the question and pull nearest chunks by
    //    meaning. Catches paraphrase + cross-language (a Serbian question hitting
    //    an English note) that keyword retrieval misses. The smarter half of the
    //    finder. (Only when built with the embedding feature.)
    #[cfg(feature = "embedding")]
    {
        use altevra_memory::{search_by_vector, AsyncEmbeddingProvider, Bge3Embedder, BGE_M3_MODEL};
        use sqlx::Row;
        if let Ok(embedder) = Bge3Embedder::new() {
            if let Ok(emb) = embedder.embed(&args.question).await {
                let ranked = search_by_vector(&pool, &emb.vector, BGE_M3_MODEL, args.limit)
                    .await
                    .unwrap_or_default();
                for (chunk_id, _score) in ranked {
                    if let Ok(Some(r)) = sqlx::query(
                        "SELECT mc.text AS text, mc.created_at AS created_at, \
                                COALESCE(md.source_path,'') AS source_path \
                         FROM memory_chunks mc LEFT JOIN memory_documents md ON md.id = mc.document_id \
                         WHERE mc.id = ?",
                    )
                    .bind(chunk_id.to_string())
                    .fetch_optional(&pool)
                    .await
                    {
                        let when = chrono::DateTime::parse_from_rfc3339(&r.get::<String, _>("created_at"))
                            .map(|d| d.with_timezone(&Utc))
                            .unwrap_or(now);
                        if !in_window(when, t_since, t_until) {
                            continue;
                        }
                        let path: String = r.get("source_path");
                        let label = path.trim_start_matches("./");
                        let snip = r.get::<String, _>("text").chars().take(400).collect::<String>();
                        ctx_lines.push(format!("[~meaning · {label}] {snip}"));
                        if !label.is_empty() {
                            sources.push(label.to_string());
                        }
                    }
                }
            }
        }
    }

    if ctx_lines.is_empty() {
        println!("No matching context in your second brain for: \"{}\"", args.question);
        return Ok(());
    }

    // --- READ + ANSWER (cheap fast model) ---
    let context = ctx_lines
        .iter()
        .take((args.limit as usize) * 3)
        .enumerate()
        .map(|(i, l)| format!("{}. {l}", i + 1))
        .collect::<Vec<_>>()
        .join("\n");

    let messages = vec![
        ChatMessage::system(
            "You are Pavle's second-brain answer engine. Answer his question USING ONLY the \
             retrieved context below — it's from his own sessions, notes, decisions, and files. \
             Be concrete and cite which snippet(s) you used (by their bracket label). If the \
             context doesn't contain the answer, say so plainly — never invent. Match Pavle's \
             language (Serbian/English). Keep it tight.",
        ),
        ChatMessage::user(format!(
            "QUESTION:\n{}\n\nRETRIEVED CONTEXT:\n{context}",
            args.question
        )),
    ];

    let provider = ClaudeCliProvider::new(&args.model);
    let answer = provider
        .complete(&messages, &ChatOpts::default().with_max_tokens(900))
        .await
        .unwrap_or_else(|e| format!("(answer failed: {e})"));

    println!("{answer}");
    if !sources.is_empty() {
        sources.sort();
        sources.dedup();
        println!("\n— sources: {}", sources.into_iter().take(6).collect::<Vec<_>>().join(", "));
    }
    Ok(())
}

fn in_window(t: DateTime<Utc>, since: Option<DateTime<Utc>>, until: Option<DateTime<Utc>>) -> bool {
    since.map(|s| t >= s).unwrap_or(true) && until.map(|u| t < u).unwrap_or(true)
}

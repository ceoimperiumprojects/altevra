---
id: wiki_project_altevra
type: wiki_page
topic: altevra
status: living
confidence: high
last_synthesized_at: 2026-05-28
source_count: 4
related_projects: []
related_pages:
  - resident-agent
  - wiki-layer
  - context-engineering
sensitivity: public
owner: altevra
---

# Altevra (project)

## Current Understanding

Altevra is Pavle Anđelković's local-first **external mind** and **AI agent operating system**. It is not a productivity tool, not a startup product, not a chat assistant — it is the memory + intelligence layer underneath every AI tool Pavle uses, designed to compound knowledge over decades.

The system is built in Rust as a workspace of 16+ crates with a single CLI binary (`altevra`) that doubles as an MCP server for any AI tool that speaks Model Context Protocol. Storage is local SQLite + filesystem; cloud sync is opt-in per category and never default.

The repository is **public** under PolyForm Strict 1.0.0 (source-available, commercial use requires a written license). The data — Pavle's life context — is **never** open: it is sovereign to its owner.

## Why It Matters

Pavle spans many projects (ReVesta, Tunia, Imperium Cockpit, CoGrader, Hermes, PhoneAgent, ImperiumCrawl, +others) and many AI tools (Claude Code, Codex CLI, Cursor CLI, Antigravity, Hermes). Without Altevra, every session starts from zero, hard-won preferences vanish, and decades of accumulated learning never compound. With Altevra, every AI tool gets bootstrap context at session start and every tool call is captured for future synthesis.

## Key Facts

- **License:** PolyForm Strict 1.0.0 (source-available, not OSI open-source)
- **Language:** Rust (workspace of 16 crates)
- **Storage:** SQLite (sessions, turns, memory, wiki, research, embeddings) + filesystem markdown (skills, wiki, configs) + OS keyring (secrets)
- **AI integration:** MCP server + per-tool adapters (Claude Code, Codex, Cursor, Antigravity, Hermes)
- **Current version:** v0.3.x — recorder + watcher + embedder + brain + research v2 + Analyze Everything (historical import) shipped
- **Test count:** 475+ passing (2026-05-28), 0 failing
- **MCP tools surfaced:** 32+ (will rise to 36+ at Phase 1 wiki/resident tools)
- **Sub-versions completed:** v0.3.2 watcher, v0.3.3 embedder, v0.3.4 brain, v0.3.5 research, v0.3.6 hook fan-out, v0.3.7 replay, v0.3.7.5 research v2, v0.3.8 Analyze Everything

## Key Decisions

- 2026-05-27 — Repo public under PolyForm Strict (not OSI open-source). Commercial license required for non-personal use.
- 2026-05-28 — Vision codified: personal + business equally first-class, designed for decades. See `VISION.md`.
- 2026-05-28 — Identity split: Hermes owns light identity, Altevra owns deep versioned identity. Hermes calls Altevra via MCP when depth is needed.
- 2026-05-28 — Self-improving architecture borrowed from Hermes (`background_review.py`) and generalized across all Altevra surfaces (imports, hooks, research, wiki). See `CLAUDE.md` §12.
- 2026-05-28 — Skill factory: Altevra generates skills for other AI tools based on observed Pavle workflows. See `VISION.md` §4.4.

## Open Questions

- v0.3.9 multi-provider LLM design: do we eagerly load all configured providers, or lazy-init on first call per role? (Lazy probably wins.)
- Local model integration via Ollama: how to handle the case where Ollama is not running? (Fallback to next role in chain.)
- Cursor CLI 50K+ ai_code_hashes: import all at v0.5 launch, or paginate? (Plan: incremental import keyed by `createdAt`.)
- Knowledge graph (Phase v0.6+): should edges live in `wiki_page_links` extended schema or separate `knowledge_edges` table? (Probably separate — edges span more than wiki.)

## Related Pages

- [[resident-agent]]
- [[wiki-layer]]
- [[context-engineering]]

## Evidence / Sources

- `VISION.md` (decade-arc vision)
- `CLAUDE.md` (operating doctrine + system architecture)
- `ROADMAP.md` (concrete build sequence Phase 0 → Phase 10)
- `ALTEVRA_NEXT_ARCHITECTURE_RESIDENT_AGENT_WIKI_PERSONAL_BRAIN.md` (next-phase technical architecture)
- `README.md` (public-facing summary)
- `BRAND.md` (visual identity)

## Last Updates

- 2026-05-28 — Phase 0 stabilization shipped (commit `3e64dd4`): clippy clean, antigravity noise eliminated, DB path consolidated, auto-symlink on `altevra init`.
- 2026-05-28 — Phase 1 wiki + resident foundation in progress (this page is part of it).
- 2026-05-28 — Vision adopted: VISION.md + ROADMAP.md + ALTEVRA_NEXT_ARCHITECTURE.md committed.

## Suggested Next Actions

- Phase 1: finish wiki frontmatter parser + CLI + MCP tools (~1–2h remaining).
- Phase 2: build `altevra-llm` multi-provider abstraction (chat + embedding, native + OpenAI-compat) so resident runtime can route by role.
- Phase 3: v0.3.10 onboarding wizard so a fresh `git clone` → working install is <10 min.

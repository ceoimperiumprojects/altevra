---
id: wiki_layer
type: wiki_page
topic: wiki-layer
status: living
confidence: high
last_synthesized_at: 2026-05-28
source_count: 2
related_projects:
  - altevra
related_pages:
  - resident-agent
  - context-engineering
sensitivity: internal
owner: altevra
---

# Wiki Layer

## Current Understanding

The Wiki Layer is Altevra's living, synthesized knowledge layer.

A wiki page is **not a raw note**. A wiki page is **what Altevra currently understands** about a topic.

```
Event log    = what happened
Memory       = what was stored
Insight      = what was noticed
Wiki page    = what is currently understood
```

Wiki pages live in `wiki/` at repo root with subfolders by entity kind:

- `concepts/` — abstract ideas (resident-agent, context-engineering, agent-os)
- `projects/` — software / business / life projects (altevra, revesta, tunia)
- `people/` — relationships (pavle, danilo, andrija)
- `patterns/` — recurring behaviors (overnight-runs, research-synthesis-loop)
- `decisions/` — material decisions with provenance (rust-first, source-available)
- `domains/` — life domains (ai-agents, foreclosure-surplus, music)

Each page has typed frontmatter (`id`, `type`, `topic`, `status`, `confidence`, `sensitivity`, `source_count`, `last_synthesized_at`, `related_projects`, `related_pages`, `owner`) plus the standard body sections defined in `ALTEVRA_NEXT_ARCHITECTURE.md` §12.5.

## Why It Matters

Without wiki pages, agents must synthesize every topic from raw context each time — expensive and inconsistent.

With wiki pages, agents can load a **compact, agent-readable** page that explains a concept, project, person, pattern, or decision in <1k tokens. This saves tokens, improves reasoning, and provides Pavle with a human-readable read-only view of "what Altevra currently believes."

Wiki pages compound: as new evidence lands (sessions, research, decisions), the wiki_curator mode (Phase 5) proposes updates. Low-risk changes auto-apply; sensitive ones queue for review.

## Key Facts

- Wiki pages are markdown with YAML frontmatter — parseable by both Obsidian and Altevra's own `altevra-vault::wiki` module.
- Wiki links use double-bracket syntax: `[[other-topic]]` (auto-extracted into `wiki_page_links` graph table).
- Page status: `living` (actively curated), `archived` (historical reference), `draft` (in-progress, not yet trusted).
- Page confidence: `low | medium | high` based on source count + recency.
- Page sensitivity: `public | internal | private | secret` — determines which LLM provider role (cheap_worker vs local_private) can read it.

## Key Decisions

- 2026-05-28 — Wiki lives in `wiki/` at repo root (not in Obsidian vault) so it ships with Altevra binary releases.
- 2026-05-28 — Wiki Curator is a resident mode, not a separate agent. Same prompt cache, same context engineering discipline.
- 2026-05-28 — Auto-apply allowed for: concepts, patterns, low-risk projects. Review required for: people, identity profile, decisions log, sensitive personal content.

## Open Questions

- Should wiki pages embed for semantic search? Yes — Phase 2/4 will embed page bodies into the same vector store as memory chunks.
- How to handle merges / splits gracefully? Initially: Wiki Curator proposes split/merge with diff; Pavle approves. Long-term: history per page in `wiki_page_history` table.
- Obsidian round-trip: should Altevra mirror wiki pages into the Obsidian vault for human-side editing? Probably yes via symlink (`~/Obsidian/Imperium/Altevra-Wiki/` → repo `wiki/`). v0.3.10 setup wizard task.

## Related Pages

- [[resident-agent]]
- [[context-engineering]]
- [[altevra]]

## Evidence / Sources

- `ALTEVRA_NEXT_ARCHITECTURE.md` §12 (Wiki Layer), §13 (Auto-Wiki Pipeline), §14 (example page)
- `CLAUDE.md` §10 (system architecture diagram — wiki sits inside Living Storage)
- `VISION.md` §3 (compounding knowledge)

## Last Updates

- 2026-05-28 — Page seeded as part of Phase 1 wiki bootstrap.

## Suggested Next Actions

- Phase 1 (this phase): wire `altevra-vault::wiki` parser + CLI `altevra wiki list/show/search` + MCP tools `get_wiki_page` / `search_wiki`.
- Phase 5: wire Wiki Curator mode end-to-end via brain job `wiki_curator_sweep`.
- v0.3.10: setup wizard offers Obsidian mirror symlink.

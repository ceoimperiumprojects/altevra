---
id: wiki_resident_agent
type: wiki_page
topic: resident-agent
status: living
confidence: high
last_synthesized_at: 2026-05-28
source_count: 2
related_projects:
  - altevra
related_pages:
  - context-engineering
  - wiki-layer
sensitivity: internal
owner: altevra
---

# Resident Agent

## Current Understanding

The Resident Agent is the internal intelligence layer inside Altevra.

It turns memory, research, sessions, tasks, goals, wiki pages, decisions, and updates into structured intelligence.

It is not a chat assistant. It is not a general agent that does everything randomly. It is a controlled internal reasoning system that runs in **modes**:

- `memory_curator` — keep memory clean, deduplicated, current
- `synthesis` — turn cleaned research into project-linked insight
- `daily_briefing` — produce daily morning brief
- `wiki_curator` — maintain living wiki pages
- `insight` — notice useful cross-context connections (Phase 4+)
- `observer` — observe Altevra usage, propose improvements (Phase 8)
- `research` — drive opt-in topic research (Phase 4+)
- `task_goal` — keep tasks/goals consistent (Phase 4+)

## Why It Matters

This is the layer that makes Altevra **think** instead of only **store**.

Without it, Altevra is a recorder. With it, Altevra becomes a compounding external mind.

It is also the layer that lets cheap models excel — by giving them a tight system prompt + mode-specific prompt + strict context packet + structured output schema, a 4B-parameter model can produce high-quality synthesis that a 70B-parameter model would produce on bloated context.

## Key Decisions

- 2026-05-28 — Single agent, multiple modes (not one god agent). Each mode has its own system prompt, allowed tools, forbidden actions, context packet shape, and output schema.
- 2026-05-28 — Context packets, not vault dumps. Resident receives a strict envelope per `context_packet_v1` and may request hydration in a second pass.
- 2026-05-28 — Model routing by role: cheap_worker for cleanup, strong_reasoner for synthesis, local_private for sensitive personal context.
- 2026-05-28 — Never overwrite source-of-truth. Resident proposes diffs; Pavle approves.

## Open Questions

- Which model should be default for synthesis on first install? Gemini Flash (free tier) is the v0.3.8 default; Phase 2 / v0.3.9 introduces routing config so DeepSeek/Qwen become drop-in alternatives.
- How often should `wiki_curator` run? Probably every 4h (configurable via brain job config) but needs Pavle's daily-usage data to tune.
- Which pages can be auto-updated without review? Concept and pattern pages, yes. People and decision pages, no — those always go through review queue.
- How much personal context should daily briefing include by default? Personal Signals section is opt-in; default is "empty unless explicit allow per category" (see `relevance gate` once Phase 6 lands).

## Related Pages

- [[context-engineering]]
- [[wiki-layer]]
- [[altevra]]

## Evidence / Sources

- `CLAUDE.md` §12 (self-improving architecture)
- `VISION.md` §4 (self-improving pattern from Hermes)
- `ALTEVRA_NEXT_ARCHITECTURE_RESIDENT_AGENT_WIKI_PERSONAL_BRAIN.md` §3–§6

## Last Updates

- 2026-05-28 — Page seeded as part of Phase 1 wiki bootstrap.

## Suggested Next Actions

- Phase 2: build multi-provider LLM crate so Phase 4 resident runtime can route by role.
- Phase 4: wire `memory_curator` end-to-end (safest mode — read-only proposals).
- Phase 5: wire `wiki_curator` to auto-update low-risk pages.

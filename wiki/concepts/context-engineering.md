---
id: wiki_context_engineering
type: wiki_page
topic: context-engineering
status: living
confidence: high
last_synthesized_at: 2026-05-28
source_count: 2
related_projects:
  - altevra
related_pages:
  - resident-agent
  - wiki-layer
sensitivity: internal
owner: altevra
---

# Context Engineering

## Current Understanding

Context engineering is the discipline of constructing prompts so that even a small/cheap model performs as well as a much larger model would on bloated context.

Altevra treats this as a first-class architectural concern. The system prompt is small. Mode-specific prompts are small. The context packet is strict. The output schema is enforced. Tools for retrieval are well-named. The model never has to "figure out what to look at."

This is why Altevra can use DeepSeek V4 Flash, Qwen Plus, MiniMax M2.7, Gemini Flash, or local Ollama Llama 3.2 as production models — they all become competent when handed clean, small, structured context. Frontier models are reserved for development/review.

## Why It Matters

Without context engineering:

- Every request hits long-context API → expensive
- Cheap models hallucinate on dump-style prompts → low quality
- No reproducibility (today's vault snapshot ≠ tomorrow's)
- Sensitive data leaks into every call

With context engineering:

- Cheap models = strong reasoners
- Outputs are structured → composable into downstream pipelines
- Reproducible (`context_packet_v1` is hashable input)
- Sensitive content routes to `local_private` role only

## Key Facts

- **System prompt is stable** — built once per session, reused across turns. Cached upstream.
- **Mode prompt is small** — under 500 words per mode, focused on one job.
- **Context packet is strict** — 14 rules in [`00-system/token-economy.md`](../../00-system/token-economy.md), enforced by builder before any LLM call.
- **Two-pass retrieval** — first pass: IDs + summaries; second pass: hydrate only chunks the model explicitly requests.
- **Output schemas are mandatory** — every mode declares `output_schema` in its frontmatter; orchestrator validates before persisting.

## Key Decisions

- 2026-05-28 — Adopt strict context packet (no whole-vault dumps).
- 2026-05-28 — Two-pass retrieval is default; one-pass only when context fits below `max_input_tokens / 4`.
- 2026-05-28 — Mode prompts live in `06-skills/resident-agent-modes/*.md` (loadable by `altevra-skills::SkillRegistry`).

## Open Questions

- What's the right `max_input_tokens` ceiling per mode? Synthesis currently set to 12000; daily_briefing probably needs less (~6000); wiki_curator on large pages may need 16000.
- Does the reranker live inside `altevra-research` or get its own crate? TBD when Phase 2 + Phase 5 surface concrete needs.
- How to instrument context packet effectiveness? Likely log `tokens_in`/`tokens_out` per resident run and watch the ratio.

## Related Pages

- [[resident-agent]]
- [[wiki-layer]]
- [[altevra]]

## Evidence / Sources

- `ALTEVRA_NEXT_ARCHITECTURE.md` §7 (Context Packet System), §8 (Token Economy), §9 (Model Routing)
- `VISION.md` §2.4 (relevance, not noise)
- `00-system/token-economy.md`

## Last Updates

- 2026-05-28 — Page seeded as part of Phase 1 wiki bootstrap.

## Suggested Next Actions

- Phase 4: implement `context_packet_v1` builder in `altevra-context` crate (or module in `altevra-bootstrap`).
- Phase 4: write per-mode budget defaults to a config file (`~/.altevra/resident.yaml`) so Pavle can tune without recompiling.

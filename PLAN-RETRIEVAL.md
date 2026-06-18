# Plan: Altevra context-aware retrieval overhaul
_Locked via grill — by Claude + Pavle, 2026-06-19. Revised after Codex round 1._

## Posture (Pavle's explicit decision)
SINGLE-USER, local-first, sovereign. Pavle is the only user AND the data owner.
Safety/exposure gating BETWEEN his own surfaces (recall/ask/MCP/packet) is DESCOPED —
there is no other party to protect from. Full content flows freely between his tools.
The one external surface (MCP → cloud tools like ChatGPT) is already authorized for full
exposure (Decisions 2026-06-18). So Codex's safety findings (#1,2,5,9,11-safety) are
out of scope BY DECISION. We keep ONLY the correctness fixes Codex found (data loss,
schema overwrite, broken fusion key, unmaintained FTS, eval-before-rewiring).

## Goal
Make Altevra retrieval best-in-class AND context-aware GENERALLY (not repo-scoped):
(1) quality — hybrid tri-signal RRF + selective enrich-before-embed + intent routing
(per `docs/RETRIEVAL-SPEC.md`); (2) **the brain always knows what Pavle is working on** —
it AGGRESSIVELY self-infers project/domain of every activity from CONTENT (not path) and
connects related work across surfaces (Skool / CRM / call transcript / Desktop file all
resolve to ReVesta automatically). No alias list — the model figures it out, freely.

## Approach
0. **Unblock the local lane (PREREQ).** `llama-3.2-1b-instruct.Q8_0` on llama-server
   Vulkan iGPU (loopback openai_compat) as a systemd user unit + health check; verify
   `local_private` resolves to it not noop (it silently noops RIGHT NOW). bge embed onto
   iGPU Vulkan (≈10× CPU).
1. **Hybrid backbone (highest leverage, mostly wiring existing code).**
   - Fix the broken fusion KEY (Codex #4): `hybrid_db_search` fuses BM25 `object_id`
     against vector hits mapped to `memory_chunks.document_id` — different key spaces.
     Resolve a canonical `(object_type, object_id)` for chunks (parse `db://` as fallback),
     fuse on ONE stable key, and (Codex #4/semantic-hole) join chunk→memory_chunks.text so
     vector-only and keyword-only hits both surface.
   - Repoint `recall`, `ask` run_lookup, and MCP `search_turns`/`search_files` at the fixed
     `hybrid_db_search`. **Stay OUT of the packet compiler / golden-eval core (Codex #10,
     R15)** — those stay vector-free until R15 is ratified separately.
   - Embedder: `embed()`→`embed_batch()` over the claimed batch; drop the dead 1000-RPM
     Gemini-era limiter for local BGE.
2. **Context-awareness + selective enrich-before-embed.** New `altevra-enricher.service`
   reads an enricher_queue. For EVERY turn/chunk it tags `{project, domain}` (AGGRESSIVE,
   content-inferred, free — own-infra/Altevra | ReVesta-biz | idea | personal). For VERBOSE
   chunks (turns/transcripts/>~1200 chars) it ALSO distills: contextual header
   (title+breadcrumb+gist, PREPENDED to the embed input) + sidecar {gist, keywords (expanded
   acronyms + sr↔en), questions-it-answers, entities} → `memory_chunk_enrichment` +
   `chunk_kw_fts` (the 3rd RRF leg). Raw text stays the citation source.
   - **Enriched REPLACES raw** (Codex #3): re-embed upserts by `chunk_id` (no PK change, no
     coexistence). Dual-mode: embed raw immediately so nothing is unsearchable; re-embed
     enriched when the sidecar lands.
   - **No data loss** (Codex #8): make a tail-summary chunk (or keep raw refs) BEFORE
     `MAX_CHUNKS_PER_TURN` truncation — never drop unseen chunks first.
   - **Maintain FTS** (Codex #7): index each chunk into `object_fts` inside `persist_document`,
     with cleanup on document replacement (otherwise the lexical leg is empty).
   - Cross-surface linking: project/entity resolution so Skool/CRM/calls/Desktop all link to
     the ReVesta node and recall/ask surface them together.
3. **Coverage ledger + new sources.** `coverage_ledger` (per-object freshness/state),
   repo-sweep over projects.yaml, **call-transcript adapter** (revesta-crm transcripts are
   NOT indexed today), freshness re-enqueue, coverage% on the daily digest.
4. **Query router + rerank.** Shared `route_and_enrich()` generalizing `is_aggregate` into
   {lookup, aggregate, entity, synthesis, temporal}; trivial-query pre-filter skips the LLM
   (zero tax on the common case); local sr↔en expansion; aggregate→window+group_by; rerank =
   HyDE-reverse (questions-vs-query, near-free), escalate to a local listwise rerank on
   ambiguity. Fail-open to gated-keyword retrieval (single-user, so quality-only concern).
5. **Recall eval baseline (build BEFORE Phase 1 rewiring — Codex #12).** A small golden set
   (recall@k / MRR over a handful of known Q→expected) so the hybrid rewiring is measured,
   not guessed. `altevra eval` reports regression. (Leak-suite descoped — single user.)
6. **Scale (deferred).** sqlite-vec/HNSW when chunk_count > ~100k; ColBERT-light + BGE-M3
   native sparse only if later verified/needed.

## Key decisions & tradeoffs
- **enrich-before-embed = YES but SELECTIVE** (verbose lane only), contextual-retrieval
  pattern (prepend header + sidecar), NEVER replace raw text. Dense objects skip it.
- **Context inference is AGGRESSIVE + free** — the model classifies project/domain and links
  cross-surface without an alias list or safety default-up (single-user; misclassification
  costs retrieval quality, not a leak).
- **Model ladder: 1B local FIRST, escalate to Haiku when 1B can't** (Pavle's call) — cheap
  per-turn tag + distillation on llama-3.2-1b (iGPU); aggressive cross-domain linking +
  ambiguous cases → Haiku. Quality of context-understanding wins over cheap.
- **Small model on iGPU/CPU, NOT NPU** — XDNA measured 10-13× slower (autoregressive gen is
  its worst case). NPU left for bulk re-embed only.
- **Hybrid = TRI-SIGNAL RRF** (object_fts BM25 + distilled-keyword BM25 + dense cosine).
  fastembed exposes DENSE only → "sparse" = distilled keywords, not BGE-M3's sparse head.
- **Enriched vectors REPLACE raw** (upsert by chunk_id) — no schema/PK change, no coexistence.

## Risks / open questions
- 1B too weak for aggressive linking → Haiku escalation + the recall baseline measures it.
- RAM ceiling ~3GiB free: 1B (1.3GB) + bge (1.1GB) + brain barely fit; lfm2.5:8b (5GB) is
  on-demand only, never co-resident.
- iGPU contention while gaming → CPU fallback (slower, not wrong); enrichment async so
  nothing is unsearchable, only enriched re-embed lags.
- Enrichment backlog (distill slower than ingest) → dual-mode mitigates (two embed passes).

## Out of scope
- Safety/exposure gating between Pavle's own surfaces (single-user sovereign decision).
- ColBERT multi-vector NOW, BGE-M3 native sparse head, generation on the NPU.
- Tunia / any non-ReVesta-non-Altevra project. Hand-maintained alias lists. Browser-action capture.

## Codex-hardened build contract (locked after 2 review rounds)
1. **Retrieval key** = `chunk:<chunk_id>` for EVERY leg (FTS + keyword + dense); carry a
   `source_ref{object_type, object_id, session_id, turn_idx, source_path, ts}` for citation
   + filters. One key space so RRF actually fuses matching evidence.
2. **Enriched re-embed queue path**: enricher resets the existing `embedder_queue` row to
   `pending` (or writes the vector directly); the worker selects the enriched input when
   `memory_chunk_enrichment.source_checksum == memory_chunks.checksum`. (seed_queue's
   skip-if-vector-exists does not block re-embed.)
3. **Tail-summary MANDATORY** before `MAX_CHUNKS_PER_TURN` truncation + keep a raw source
   ref for citation + a test proving content after chunk 8 is retrievable.
4. **Replacement cleanup in ONE transaction**: delete queue + vector + enrichment +
   chunk_kw_fts + object_fts rows for the old chunk ids.
5. **`altevra eval gate` EXITS NON-ZERO** on regression and is REQUIRED before rewiring
   recall/ask/MCP.
6. **Typed `RetrievalRequest`** {query, project?, tool?, since?, until?, limit} + a resolver
   returning session_id/turn_idx/source_path/timestamps/parent metadata — the hybrid path
   must NOT lose the filters/provenance that `search_turns` has today.
7. **Phase 0 wires `Bge3Embedder`** as the actual embed provider (embed.rs today imports
   only Gemini/NoOp) BEFORE any batch/limiter change.

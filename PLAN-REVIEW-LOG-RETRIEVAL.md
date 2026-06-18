# Plan Review Log: Altevra context-aware retrieval overhaul
Act 1 (grill) complete — plan locked with Pavle. MAX_ROUNDS=2.

## Round 1 — Codex
**Findings**

1. **SI-7 is not preserved.** The plan allows ambiguous cross-domain linking/classification to escalate to Haiku, exactly when content may be personal/high-water ([PLAN-RETRIEVAL.md](/home/pavle/projekti/ai-tooling/altevra/PLAN-RETRIEVAL.md:54), [PLAN-RETRIEVAL.md](/home/pavle/projekti/ai-tooling/altevra/PLAN-RETRIEVAL.md:69)). Fix: ambiguity must default-up to `restricted` + `local_private`; cloud only after a local pre-scan proves non-high-water.

2. **“Fail-open everywhere” violates the retrieval/safety contract.** The plan/spec say router/rerank/enrichment fail open ([PLAN-RETRIEVAL.md](/home/pavle/projekti/ai-tooling/altevra/PLAN-RETRIEVAL.md:42), [docs/RETRIEVAL-SPEC.md](/home/pavle/projekti/ai-tooling/altevra/docs/RETRIEVAL-SPEC.md:81)), but retrieval exposure must fail closed on safety uncertainty. Fix: fail open only to deterministic already-gated retrieval; for high-water, enrichment/rerank/router failure must skip or local-only fallback.

3. **Raw+enriched vectors cannot coexist as written.** `memory_chunk_vectors_v2` is `PRIMARY KEY(chunk_id)` ([010_embedder_queue.sql](/home/pavle/projekti/ai-tooling/altevra/crates/altevra-db/migrations/010_embedder_queue.sql:20)) and upsert overwrites by `chunk_id` ([vector_store.rs](/home/pavle/projekti/ai-tooling/altevra/crates/altevra-memory/src/vector_store.rs:87)). Fix: migrate to `PRIMARY KEY(chunk_id, model)` or add `lane/vector_id`, then update seed/search/import/export semantics.

4. **The hybrid fusion key is wrong.** `hybrid_db_search` fuses BM25 `object_id`s with vector hits mapped only to `memory_chunks.document_id` ([hybrid_db.rs](/home/pavle/projekti/ai-tooling/altevra/crates/altevra-memory/src/hybrid_db.rs:64), [hybrid_db.rs](/home/pavle/projekti/ai-tooling/altevra/crates/altevra-memory/src/hybrid_db.rs:190)); that is a memory document UUID, not necessarily the source object. Fix: store/resolve canonical `(object_type, object_id)` on indexed documents/chunks, parse `db://` only as fallback, and fuse on one stable key type.

5. **Repointing MCP to `hybrid_db_search` would bypass the packet compiler.** `DbHybridHit` has no sensitivity/domain/redaction fields ([hybrid_db.rs](/home/pavle/projekti/ai-tooling/altevra/crates/altevra-memory/src/hybrid_db.rs:17)), while the existing packet path gates via `object_index` + `ExposureGate` ([packet_build.rs](/home/pavle/projekti/ai-tooling/altevra/crates/altevra-mcp/src/packet_build.rs:17)). Fix: create a gated hybrid retrieval API that joins safety metadata, applies `ExposureGate` before scoring, and writes exposure audit.

6. **Memory document safety metadata is not updated by indexing.** The worker calls `guard_document` but ignores its returned sensitivity and inserts/updates `memory_documents` without `sensitivity` or `redaction_status`, leaving defaults like `unscanned` ([worker.rs](/home/pavle/projekti/ai-tooling/altevra/crates/altevra-memory/src/worker.rs:224), [worker.rs](/home/pavle/projekti/ai-tooling/altevra/crates/altevra-memory/src/worker.rs:335), [019_object_envelope.sql](/home/pavle/projekti/ai-tooling/altevra/crates/altevra-db/migrations/019_object_envelope.sql:77)). Fix: persist guard verdict, domain, policy version, and redaction status in the same transaction as document/chunk writes.

7. **`object_fts` is not maintained for `memory_chunks` by the worker.** The helper exists ([ingestion.rs](/home/pavle/projekti/ai-tooling/altevra/crates/altevra-memory/src/ingestion.rs:121)), but `persist_document` only inserts chunks and queue rows ([worker.rs](/home/pavle/projekti/ai-tooling/altevra/crates/altevra-memory/src/worker.rs:349)). Fix: index each guarded chunk into FTS during `persist_document`, with cleanup on document replacement.

8. **Overflow distillation cannot recover data after truncation.** The current DB-object path truncates turn chunks before persistence ([worker.rs](/home/pavle/projekti/ai-tooling/altevra/crates/altevra-memory/src/worker.rs:267)), while the plan proposes later distilling overflow ([PLAN-RETRIEVAL.md](/home/pavle/projekti/ai-tooling/altevra/PLAN-RETRIEVAL.md:32)). Fix: preserve overflow as raw refs or create a tail-summary chunk before truncating; never drop unseen chunks first.

9. **The no-redaction `search_files` exception is a privacy hole for MCP.** Existing MCP exposes indexed file text without gate ([tools_sessions.rs](/home/pavle/projekti/ai-tooling/altevra/crates/altevra-mcp/src/tools_sessions.rs:209)), and the spec preserves it ([docs/RETRIEVAL-SPEC.md](/home/pavle/projekti/ai-tooling/altevra/docs/RETRIEVAL-SPEC.md:40)). Fix: make file search use the same exposure gate by default; require explicit human/elevated audited mode for raw local file recall.

10. **The plan contradicts current architecture R15.** R15 says packet compiler/golden eval stay vector-free and semantic hybrid is opt-in above core ([RECONCILIATION.md](/home/pavle/projekti/ai-tooling/altevra/docs/architecture/RECONCILIATION.md:273)). The plan moves dense retrieval into `get_context_packet` and eval. Fix: update/ratify the architecture first, or keep Phase 1 outside packet compiler.

11. **“Aggressive self-learned, no alias list” is not a durable project/domain contract.** It asks a model to infer authoritative project/domain for every chunk ([PLAN-RETRIEVAL.md](/home/pavle/projekti/ai-tooling/altevra/PLAN-RETRIEVAL.md:52)), but domains are governed safety inputs. Fix: model outputs should be candidate labels with confidence/evidence; canonical project/domain changes require deterministic constraints, default-up uncertainty, and review/audit.

12. **Eval gate is too late for the riskiest change.** Phase 1 repoints retrieval surfaces, while the gate is “parallel”/retroactive ([PLAN-RETRIEVAL.md](/home/pavle/projekti/ai-tooling/altevra/PLAN-RETRIEVAL.md:43), [docs/RETRIEVAL-SPEC.md](/home/pavle/projekti/ai-tooling/altevra/docs/RETRIEVAL-SPEC.md:89)). Fix: build the minimal leak suite + recall baseline before any MCP/packet rewiring.

No files modified.

VERDICT: REVISE
### Claude's response (round 1)
Accepted nearly all — they materially harden safety + correctness:
- #1 SI-7: ambiguity defaults-up restricted+local; cloud only after local pre-scan proves non-high-water.
- #2 fail-open: retrieval QUALITY degrades to gated keyword; SAFETY (high-water enrich/rerank/router) fails CLOSED (skip/local-only).
- #3 vectors: enriched REPLACES raw via upsert(chunk_id) — drop "coexist", no PK migration needed.
- #4 fusion key: resolve canonical (object_type,object_id); db:// fallback; fuse on one stable key.
- #5 MCP gate: new GATED hybrid retrieval API joins sensitivity/domain/redaction + applies ExposureGate before scoring + writes exposure audit; MCP uses it (never raw DbHybridHit).
- #6 guard verdict: persist sensitivity/domain/redaction/policy in the SAME tx as doc/chunk writes (fixes the 'all unscanned' bug).
- #7 chunk FTS: index each guarded chunk into object_fts in persist_document; cleanup on replace.
- #8 overflow: tail-summary (or raw refs) BEFORE truncation; never drop unseen chunks first.
- #10 R15: Phase 1 stays OUTSIDE the packet compiler / golden-eval core; wire recall/ask + gated MCP retrieval only. Ratify R15 before touching packet compiler.
- #11 domain: model emits CANDIDATE project/domain labels with confidence+evidence (for retrieval/linking); the SAFETY sensitivity/domain defaults-up on uncertainty and is NEVER lowered by the model. Reconciles "aggressive self-infer" with SI-7.
- #12 eval: build the minimal leak-suite + recall baseline FIRST, before any surface rewiring.

Nuanced #9: search_files full exposure is Pavle's explicit sovereign decision (his own data, recorded in Decisions 2026-06-18) — KEEP it, but make it an explicit, AUDITED, config-flagged mode (not a silent default), and route through the gate when the flag is off.

## Round 2 — Codex
**Findings**

1. **#4 is still not actually specified enough to work.** The plan says vector chunks resolve to canonical `(object_type, object_id)` ([PLAN-RETRIEVAL.md](/home/pavle/projekti/ai-tooling/altevra/PLAN-RETRIEVAL.md:27)), but also says every chunk is indexed into `object_fts` ([PLAN-RETRIEVAL.md](/home/pavle/projekti/ai-tooling/altevra/PLAN-RETRIEVAL.md:49)); if FTS uses `memory_chunk:<chunk_id>` and dense uses `turn:<turn_id>`/`document:<id>`, RRF still won’t fuse matching evidence. Fix: define the retrieval key explicitly as either chunk-level or source-object-level for all legs; best fit is `chunk:<chunk_id>` with separate `source_ref` for parent object/citation.

2. **#3 avoids the PK conflict, but enriched re-embed still has no runnable queue path.** `embedder_queue` is keyed by `chunk_id` and done rows block re-enqueue ([010_embedder_queue.sql](/home/pavle/projekti/ai-tooling/altevra/crates/altevra-db/migrations/010_embedder_queue.sql:5)); `seed_queue` skips chunks with any vector ([worker.rs](/home/pavle/projekti/ai-tooling/altevra/crates/altevra-memory/src/worker.rs:115)). Fix: specify that enricher resets the existing queue row to `pending` or writes vectors directly, and that the embed worker chooses enriched input when `memory_chunk_enrichment.source_checksum = memory_chunks.checksum`.

3. **#8 is only partially addressed.** “Tail-summary chunk (or keep raw refs)” before truncation ([PLAN-RETRIEVAL.md](/home/pavle/projekti/ai-tooling/altevra/PLAN-RETRIEVAL.md:47)) is too loose; keeping refs does not make dropped content searchable, and current code truncates before persist ([worker.rs](/home/pavle/projekti/ai-tooling/altevra/crates/altevra-memory/src/worker.rs:267)). Fix: make tail-summary chunk mandatory before truncation, keep a raw source ref for citation, and add a test proving content after chunk 8 is retrievable.

4. **#7 is addressed for base chunk FTS, but not for new enrichment tables.** The plan adds cleanup for `object_fts` on document replacement ([PLAN-RETRIEVAL.md](/home/pavle/projekti/ai-tooling/altevra/PLAN-RETRIEVAL.md:49)), but Phase 2 also adds `memory_chunk_enrichment` and `chunk_kw_fts` ([PLAN-RETRIEVAL.md](/home/pavle/projekti/ai-tooling/altevra/PLAN-RETRIEVAL.md:42)); stale keyword/entity rows will survive chunk replacement unless manually deleted. Fix: cleanup must delete queue/vector/enrichment/chunk_kw_fts/object_fts rows for old chunk IDs in one transaction.

5. **#12 is improved but still not a gate.** The plan now says eval baseline before Phase 1, but only says `altevra eval` “reports regression” ([PLAN-RETRIEVAL.md](/home/pavle/projekti/ai-tooling/altevra/PLAN-RETRIEVAL.md:61)). Fix: make `altevra eval gate` exit non-zero and require it before rewiring `recall`, `ask`, or MCP.

6. **New correctness issue: `hybrid_db_search` cannot replace current callers without losing filters and provenance.** Existing `search_turns` has project/tool/time-window filters and returns session/turn provenance ([sessions.rs](/home/pavle/projekti/ai-tooling/altevra/crates/altevra-db/src/repositories/sessions.rs:569)); `hybrid_db_search` only accepts query/model/dim/limit and returns title/snippet ([hybrid_db.rs](/home/pavle/projekti/ai-tooling/altevra/crates/altevra-memory/src/hybrid_db.rs:42)). Fix: add a typed retrieval request with source filters and a resolver that returns `session_id`, `turn_idx`, `source_path`, timestamps, and parent metadata.

7. **New feasibility issue: the local BGE worker path is not wired.** The CLI embed command currently imports Gemini/NoOp providers, not `Bge3Embedder` ([embed.rs](/home/pavle/projekti/ai-tooling/altevra/crates/altevra-cli/src/commands/embed.rs:13)), while the plan assumes local BGE/iGPU is the active worker ([PLAN-RETRIEVAL.md](/home/pavle/projekti/ai-tooling/altevra/PLAN-RETRIEVAL.md:22)). Fix: add explicit provider selection/config for local BGE before relying on batch embedding or removing the Gemini limiter.

VERDICT: REVISE
### Claude's response (round 2) — CONVERGED (arbiter)
All 7 round-2 findings ACCEPTED — they are implementation-precision, not architecture
changes. The approach survived both rounds; these lock the HOW. Folded into the plan as a
"Codex-hardened build contract". Per Pavle's directive (2 rounds then build), proceeding to
implement. Stopping at MAX_ROUNDS=2 with the contract as the spec:
1. Retrieval key = chunk:<chunk_id> for ALL legs; carry source_ref{object_type,object_id,session_id,turn_idx,source_path,ts} for citation+filters.
2. Enriched re-embed: enricher resets embedder_queue row -> pending (or writes vector directly); worker picks enriched input when memory_chunk_enrichment.source_checksum == memory_chunks.checksum.
3. Tail-summary chunk MANDATORY before MAX_CHUNKS_PER_TURN truncation + keep raw source ref + a test proving post-chunk-8 content is retrievable.
4. On chunk/document replacement: delete queue + vector + enrichment + chunk_kw_fts + object_fts rows for old chunk ids in ONE transaction.
5. `altevra eval gate` EXITS NON-ZERO on regression and is REQUIRED before rewiring recall/ask/MCP.
6. New typed RetrievalRequest{query, project?, tool?, since?, until?, limit} + resolver returning session_id/turn_idx/source_path/timestamps/parent metadata — hybrid path must NOT lose filters/provenance vs search_turns.
7. Phase 0 MUST wire Bge3Embedder as the embed provider (embed.rs currently only Gemini/NoOp) before batch/limiter changes.

Reason this is "build now" not "deadlock": no DISAGREEMENT remains — every finding accepted. Codex's REVISE is "tighten these details," which the contract does.

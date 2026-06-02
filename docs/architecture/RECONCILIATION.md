# Altevra Architecture Reconciliation — Locked Decisions

Status: **decisions-locked / pending Pavle final sign-off**
Date: 2026-06-01
Author: Claude (Opus 4.8, 1M) — independent review after reading all 7 working-draft sections, the constitution, the review log, the P0 contract sketches, and the live code (15 crates, 18 migrations).
Supersedes the "unresolved questions" blocks across §1–§6 and the enum sketches in `contracts/P0_CONTRACTS.md`.

> Purpose: the working draft is excellent but carries **hard contradictions** between sections (and against the live code) that block P0.1. This file resolves every one of them with a single ruling, grounded in what the code already does. Where a ruling changes code, the affected crate/migration is named. After Pavle signs off, this is the authority; sections become reference.

---

## How to read a ruling

Each ruling has: **DECISION** (the lock), **WHY** (rationale), **CODE IMPACT** (what changes in the repo), **P0 phase** (when).

---

## R1 — Sensitivity model (the #1 blocker: 3 conflicting definitions)

**Conflict:**
- Live code `altevra-core/src/security.rs:3`: `Public < Internal < Confidential < Secret` — **4 levels, no `shareable`, no `restricted`**.
- §2.4: `public < internal < confidential < secret < restricted` (5) **+ orthogonal `tags` + `domains` = a lattice**.
- §1.4.7 / §6.4: `public < shareable < internal < confidential < secret < restricted` (6, total order).

**DECISION:**
1. **Canonical ladder = the §1 six-level total order**, everywhere (code, §2, §3, §6):
   `public < shareable < internal < confidential < secret < restricted`.
2. **Sensitivity is a `level` (total-ordered, 6) PLUS orthogonal metadata** that lives on the object, not inside the scalar:
   - `sensitivity_level` (the 6-ladder) — this is the ONLY thing a ceiling `≤` compares.
   - `domains[]` (governed enum, §6) — filtered by a **separate** gate condition (`domains ⊆ caller_allowed`).
   - `risk_tags[]` (`financial, health, relationship, legal, credential, identity, minor, third_party_pii`) — orthogonal flags that can *force* a review or raise the level, never themselves a ladder.
   This reconciles §1 (scalar `≤` works) with §2 (lattice behaviour) — the lattice is `level × domains × tags`, but **ceiling math only touches `level`**. `combine()` = `max(level), ∪ domains, ∪ tags` (monotone).
3. **`secret` level is reserved for credential-class only.** No durable object *body* is ever classified `secret`; only a `secret_sighting` fingerprint row may carry it. Kept in the ladder for total-order completeness (so `confidential < secret < restricted` holds), with a write invariant: "a non-`secret_sighting` object resolving to `secret` is rejected to quarantine."
4. **Open enum:** add `Other(String)` tolerant parse (mirrors live `WikiStatus::Other`), flagged for review on read — never panic.

**WHY:** §6's own Q-LADDER recommendation, and it's the only model where a single `≤` is well-defined while §2's domain/tag protections survive. Putting domains/tags *inside* the scalar (a true lattice with no total order) breaks every `sensitivity_ceiling` comparison in §1/§3.

**CODE IMPACT:** `altevra-core/src/security.rs` — extend `Sensitivity` enum: insert `Shareable` (between `Public` and `Internal`), append `Restricted` (top), add `Other(String)`. Add `RiskTag` enum + `Domain` enum (new, see R3). Update `Display`/`FromStr`. Existing rows default to `Internal` (unchanged meaning). Additive, non-breaking — `internal`/`confidential`/`secret`/`public` strings keep parsing.

**P0 phase:** P0.0 (enum) + P0.1 (column + backfill).

---

## R2 — Status enum separation (the conflation in P0_CONTRACTS)

**Conflict:** Hermes `P0_CONTRACTS.md §2` put `quarantined` inside `ObjectStatus`. But in §2, `quarantined` is a value of **`redaction_status`** (a text-scanning result), not an object lifecycle state. Two different concepts collapsed into one column = exactly the enum-overlap the consistency-breaker warned against.

**DECISION:** Five **separate** enums, never overloaded into one column, linked by invariants:

| Enum | Values | Stored where |
|---|---|---|
| `ObjectStatus` (lifecycle) | `draft, active, superseded, archived, forgotten, deleted_tombstone` | every durable row, `status` column |
| `RedactionStatus` (safety, §2) | `unscanned, clean, redacted, quarantined, rejected` | text-bearing rows, separate `redaction_status` column |
| `ReviewStatus` | `not_required, pending_review, approved, rejected, needs_changes, expired` | review-bearing rows |
| `LifecycleState` (derived, §6) | `fresh, due_for_review, expired, archived, retention_due, delete_due, legal_hold` | **computed, not stored** (except `archived` which is an `ObjectStatus`) |
| `CapabilityState` (§5) | `discovered, installed, current, outdated, drifted, broken, disabled, needs_review, missing, conflicted, unsupported` | `installed_component.status` |
| `ProposalStatus` (§4) | `proposed, triaged, approved, applied, rejected, superseded, withdrawn, deprecated` | proposal rows |

`forgotten` IS a legit `ObjectStatus` (§2.8 soft-forget). `quarantined` is NOT — it's `RedactionStatus`.

**WHY:** Each workflow has its own state machine; cramming them into one `status` string makes illegal transitions unpreventable.

**CODE IMPACT:** New enums in `altevra-core` (alongside existing `EventStatus`, `WikiStatus`). `P0_CONTRACTS.md §2` corrected (remove `quarantined` from ObjectStatus; add the RedactionStatus row). Live `installed_components.status` (currently only ever `'current'`) gets the typed `CapabilityState` machine, computed by `verify` (§5 T8), never asserted.

**P0 phase:** P0.0 (enums) + per-phase as objects land.

---

## R3 — `domain` field (does not exist in code at all)

**Conflict:** §1/§6 make `domain` a load-bearing mandatory envelope field with a governed 9-value enum. The live code has **no `domain` concept** anywhere — only `Sensitivity`. Every gap-object and every cross-domain leak defense depends on it.

**DECISION:**
1. Add a governed `Domain` enum (closed set, `Other(String)` tolerant): `business, personal, project, client, relationship, health, legal, financial, public`. Adding a domain is review-gated (§6.3, mints a `domain_policy`).
2. `domains[]` multi-value supported; `domain` = primary. Multi-domain resolution = **most-restrictive** across members (R1 `combine` for level/sync/mirror/TTL). Matches §6.4 + §1 I6.
3. `scope` = `project_id | global` (flat for P0.1). **Parent-scope hierarchy (R7) is additive, P0.4.**

**CODE IMPACT:** `Domain` enum in `altevra-core`. `domain` + `scope` columns in envelope migration (R8). Legacy rows backfill: `business` / `internal` / `global`.

**P0 phase:** P0.0 (enum) + P0.1 (column) + P0.8 (`domain_policy` table + 9-builtin seed).

---

## R4 — Human-presence authentication (Q2.19#7 / Q-HOLD — undefined, load-bearing)

**Conflict:** Every review gate (§2.9, §4.6 Tier-1/2, §5.4.5 grants, §6 policy/forget/legal-hold) requires that "approved by Pavle" cannot be a payload flag an agent sets. The mechanism was never chosen → the entire review-gate is theoretical.

**DECISION (P0 mechanism, implementable now):**
1. **TTY presence is the P0 human-presence signal.** `altevra review approve|reject`, `connect`, `grant approve`, `forget --execute`, `domain set-policy`, `legal-hold`, `export --raw` all check `std::io::IsTerminal` on stdin. Non-TTY caller → **refused** with `requires_human_presence`.
2. **`ALTEVRA_UNLOCK` one-shot env token** for legitimate non-interactive Pavle (e.g. driving from Hermes): a short-lived token Pavle sets, consumed once, audited. Absent that, non-TTY = refused.
3. **MCP/agent callers can NEVER approve.** Hard boundary in code: the MCP server has no approve/apply/grant/forget-execute path at all (only `create_review_item` / `propose_*`). This is invariant `HP-1`.
4. `"approved"` is **never** an accepted input field on any object — approval is recorded by the core after a presence check, with `decided_by` + `decided_at` + presence-method. Invariant `HP-2`.
5. **P1 enhancement (deferred):** signed passphrase-derived unlock token replacing the env token. P0 ships TTY + env token.

**WHY:** TTY check is trivial in Rust (`IsTerminal`), can't be forged by an in-process MCP handler, and unblocks every gate today. Cryptographic signing is the right long-term answer but not a P0 blocker.

**CODE IMPACT:** New `altevra-core::presence` module (`fn require_human_presence(ctx) -> Result<PresenceProof>`). Wired into all review/grant/forget/policy CLI verbs. MCP server explicitly lacks these verbs.

**P0 phase:** P0.1 (the gate must exist before the first review_item is created).

---

## R5 — `context_packet` retention conflict (§6 purges it, §3/§2 keep it)

**Conflict:** §6.5 lists `expired context_packet` under `ephemeral → auto-hard-purge`. §3.11 says the packet audit is durable for replay/forensics; §2.10 requires an append-only audit ("why was X exposed").

**DECISION:** Split the packet into two objects:
1. `context_packet` — the compiled body/items (the cache). **Ephemeral**, auto-purge at 14d (R-EPH). Reproducible from refs + `db_snapshot`.
2. `exposure_decision` (the audit row, §2.10) — `{packet_id, request, ceiling, included_refs, excluded_refs+reasons, redaction counts, db_snapshot, created_at}`. **Append-only, never auto-purged.** This is the forensic trail.
   `context_packet_sources` (the ref list) is part of the audit, not the cache.

So purging an expired packet body never destroys the "why was X exposed" record — only the regenerable cache. Resolves §6.5 vs §3.11 vs §2.10.

**CODE IMPACT:** Two tables in P0.1 (`context_packet` ephemeral + `exposure_decision` append-only). `altevra-brain` lifecycle job purges only the former.

**P0 phase:** P0.1 (both tables) + P0.8 (purge job).

---

## R6 — Embedding/vector deferral vs §3 golden eval

**Conflict:** P0_IMPL defers vector backend; §3 golden eval (15 cases) assumes embeddings.

**DECISION:** Split the golden eval by phase. The packet compiler is built backend-neutral (R already in §3) and degrades to BM25+structured+graph when `w_emb` is unavailable.

- **P0.1 golden subset (no embeddings — BM25/structured/graph/recency only):**
  `G01 bootstrap`, `G02 superseded`, `G03 personal-leak` (sensitivity gate, no embed needed), `G04 duplicate` (structural dedup), `G05 cross-project graph`, `G07 budget-squeeze`, `G08 determinism`, `G09 fake-secret` (redaction, no embed), `G10 empty-valid`, `G14 provenance-weighting`, `G15 history-intent`.
- **P0.4 (requires vector index):** `G06 stale-research-semantic`, `G11 person-lookup-elevated` (semantic), `G12 embedding-unavailable` (tests the degradation path itself), `G13 multilingual` (needs embeddings).

Leak suites that must be **0** are all in the P0.1 set (G03/G09) — security never waits for vectors.

**CODE IMPACT:** `altevra-memory` already has `embedder_queue` + `memory_chunk_vectors_v2` (migrations 010/007); the P0.1 compiler ignores them. P0.4 wires sqlite-vec.

**P0 phase:** P0.1 (subset) / P0.4 (full).

---

## R7 — Scope hierarchy (§1 flat vs §3 parent-factor)

**Conflict:** §1.4.8 scope is flat (`project_id | global`); §3.3 uses `scope_parent_factor` over a project→parent→global hierarchy.

**DECISION:** Flat scope for P0.1 (`scope_parent_factor = 0`, no parent). Add a `project` object with optional `parent_id` in **P0.8** (project lifecycle); §3's parent multiplier activates then. Not a P0.1 blocker — the compiler treats absent parent as "no parent-scope candidates."

**CODE IMPACT:** `project` table (P0.8) carries `parent_id TEXT NULL`. §3 compiler reads it if present.

**P0 phase:** P0.4/P0.8.

---

## R8 — Object envelope migration strategy (the big schema move)

**Finding:** The envelope fields `schema_version, provenance, domain, scope, tags, categories, supersedes, superseded_by, valid_until, review_after, revision, origin_device` are **absent from all 18 live tables**. Present widely: `id`, `created_at`, `status`, `metadata(JSON)`; partially: `updated_at`, `sensitivity`, `checksum`.

**DECISION:**
1. **Additive, non-breaking backfill** (FM-12 across sections). One migration (`019_object_envelope.sql`) adds the missing columns to every durable table with safe defaults: `schema_version=1`, `domain='business'`, `scope=NULL` (global), `sensitivity='internal'` where absent, `provenance` JSON `{"origin":"imported"}`, `revision=1`, empty `tags`/`categories`. No table rewrite; SQLite `ALTER TABLE ADD COLUMN` (the pattern all 18 migrations already use).
2. **Envelope conformance meta-test** (§1.14.2): enumerate every durable table, assert Required columns exist with correct affinity. This is the executable form of Law 1 and the P0.1 acceptance spine.
3. **A lightweight denormalized `object_index`** (§1 Q2 — DECISION: **build it in P0.1**): `(type, id, status, sensitivity_level, domain, scope, updated_at, title)` — one gate point for default-safe cross-type reads (I12), cheaply maintained on write.
4. **`relations` edge table** (§1.6) lands in P0.1, generalizing `wiki_page_links` (which becomes a view/migration target).

**CODE IMPACT:** `migration 019` + envelope helpers in `altevra-core` (a `Envelope` struct + trait `HasEnvelope`). Repositories extended to read/write envelope columns. `repository_roundtrip.rs` gains the conformance meta-test.

**P0 phase:** P0.1.

---

## R9 — Gap objects (tables that don't exist yet)

**Finding:** These types are referenced by the architecture but have **no migration**: `insight_card, learning, preference, person, relationship, event_log_personal, skill_proposal, prompt_proposal, capability_record, adapter_dossier, capability_grant, domain_policy, proposal, improvement_signal, prompt, prompt_eval_result, resident_mode, resident_budget, context_packet, context_packet_sources/exposure_decision, secret_sighting`.

**DECISION — land them by the phase that first writes them, not all at once:**
- **P0.1:** `relations`, `object_index`, `context_packet`, `exposure_decision`, `secret_sighting`, `review_item` envelope upgrade (table exists, add envelope). Minimal object types needed for the vertical loop: `decision`/`task` (exist) + one personal-sensitive type (`event_log_personal` or `learning`).
- **P0.2:** `adapter_dossier`, `capability_record`, `skill_proposal`, `capability_grant`.
- **P0.6:** `proposal` (unified, `kind` discriminator — §4.19#3 DECISION: **one table**), `improvement_signal`, `prompt`, `prompt_eval_result`, `resident_mode`, `resident_budget`.
- **P0.8:** `domain_policy` (+9 builtin seed), `project`, `person`/`relationship`/`preference` (personal brain).

**CODE IMPACT:** One migration per phase-batch, each with full envelope + roundtrip test (no raw secrets in fixtures).

---

## R10 — Misc unresolved questions (ruled quickly)

| Q | Source | DECISION |
|---|---|---|
| Storage engine | §1/§2/§3/§5 + V5 | **SQLite local-first canonical** (code already is). Postgres/pgvector = future opt-in adapter. **Action: stamp `ALTEVRA_PRODUCTION_ARCHITECTURE_V5.md` with a superseded header** so its Postgres text stops misleading. |
| `id` scheme (§1 Q1) | §1 | **UUIDv4 everywhere** (code already uses `uuid::v4`). ULID deferred; not a blocker. |
| `object_index` (§1 Q2) | §1 | **Build in P0.1** (see R8.3). |
| `decisions.status` (§1 Q8) | §1 | **Additive `status` column, backfill `accepted`/`active`.** |
| `resident_run` vs `brain_jobs` (§4.19#2) | §4 | **Extend `brain_jobs`** with §4.2.2 columns (one history table). Live scheduler/dispatch reused. |
| Proposal super-family (§4.19#3) | §4 | **One `proposal` table, `kind` discriminator.** |
| Resident executor (§4.19#1) | §4 | **noop/stub provider** behind `altevra-llm` role routing for P0.5 tests; real providers post-P0. |
| Hermes fork ↔ Altevra queue (§4.19#6) | §4 | **Separate stores; Hermes writes proposals into Altevra via MCP `propose_improvement`.** Altevra = proposal system-of-record. |
| `skill_proposal` ownership (§5 Q2) | §5 | **Co-own:** §4 generation+status, §5 render+install+usage+deprecate. |
| Fold `skill_installation` (§5 Q3) | §5 | **Fold into `installed_component(component_type=skill)`;** `skill_installations` becomes legacy view/migration target. |
| `Ahead` component state (§5 Q1) | §5 | **`conflicted` → review, never auto-clobber.** |
| Hermes as render target (§5 Q7) | §5 | **Yes — Hermes gets a `ToolAdapter`** (skills → `~/.imperium/skills/shared/`). Cross-agent sharing symmetric. |
| Two-vault canonicity (Q-VAULT) | §6 | **Altevra machine vault** (`~/.altevra/vault`, numbered zones) is machine-managed; **Imperium vault** (`~/Obsidian/Imperium/`) stays human-canonical; Altevra writes Imperium **only as `generated_mirror`**, never authoritative. |
| Project object source (Q-PROJ) | §6 | **`~/.imperium/identity/projects.yaml` canonical;** brain mirrors as `imported_readonly` `project` objects carrying live `status`. |
| Financial/legal retention (Q-FIN) | §6 | **7y→review (no auto-purge);** confirm per-jurisdiction (Serbia personal vs Wyoming LLC) later — Pavle. |
| Ephemeral purge horizons (Q-EPH / R-EPH) | §6 | session `turn`s compacted-then-`archived` on session summary; low-importance `system_event` purge 90d; dismissed `research_item` 30d; `context_packet` body 14d; `embedder_queue` on done. |
| Legal-hold authority (Q-HOLD) | §6 | **Human-presence (Pavle) only** (R4); agents propose only. |

---

## R11 — Process gap: Codex skeptical review never ran

**Finding:** Codex breakers (security/consistency/product/implementation) were blocked out-of-credits; Hermes ran a *fallback* review of work Hermes itself synthesized. Independent red-team on §2 (safety) and §4 (firewall) — where a bug = leak of Pavle's personal/health/relationship data — never happened.

**DECISION:** **Before merging the P0.1 safety code** (`PreWriteSafetyGate`/`ExposureGate`), re-run the Codex breakers (credits reset). This is a gate on P0.1 *safety* sign-off, not on P0.0 contract work — contracts can proceed in parallel. The reconciliation above is mine (independent of Hermes), which partially fills the gap, but a true adversarial pass on the safety crate is still required.

**P0 phase:** gate before P0.1 safety merge.

---

## R12 — Tag-first retrieval; NO semantic search in the core path (Pavle directive 2026-06-01)

**Directive:** Pavle: *"nećemo praviti semantic search ... to je drkanje [re-embedding friction]. Treba mi da i agent i Altevra mogu lako da pretražuju — sve lako tagovano."*

**DECISION — this overrides R6 and §3's embedding-centric design:**
1. **Primary retrieval = three deterministic index families, NO vector:**
   - **Tag/structured index** (PRIMARY) — governed tags + envelope filters (`type, domain, scope, status, sensitivity_level, categories`). This is the main way both external agents and Altevra's resident modes find things.
   - **Lexical BM25** — SQLite **FTS5** full-text over `title + body + tags` (handles SR + EN via analyzer; no model needed).
   - **Graph** — typed `relations` edges (§1.6) for "what connects to what."
2. **Embeddings / vector search = OPTIONAL P1 add-on**, never in the P0 core retrieval path. No model dependency to search. The `embedder_queue`/`memory_chunk_vectors_v2` tables stay dormant; nothing in P0 reads them for retrieval.
3. **Determinism is free** — without vectors, packets are trivially byte-deterministic; the §3.3 two-layer model (gates ≠ weights) stays, but the "relevance" layer is `bm25 + tag-match + graph + recency`, no `s_emb`.
4. **Golden eval (supersedes R6 split):** drop the embedding-dependent cases (G06-semantic, G11-semantic, G12-embedding-unavailable, G13-via-embeddings). Multilingual (G13) is handled by the **FTS5 analyzer + mandatory tags**, not embeddings. All other golden cases (G01–G05, G07–G10, G14, G15) run in P0.1 with full leak suites = 0.

**WHY:** Matches local-first/no-model-dependency doctrine, kills the embedding maintenance friction Pavle rejects, makes retrieval deterministic and debuggable, and makes the **mandatory-tag system (R13) the load-bearing search substrate** rather than a fuzzy vector space.

**CODE IMPACT:** `altevra-memory` FTS5 setup; packet compiler (T1.15) drops the embedding signal entirely (was P0.4 T4.x — now P1). `altevra-db` adds FTS5 virtual tables.

**P0 phase:** P0.1 (tag+FTS5+graph retrieval is now the *full* retrieval, not a subset).

---

## R13 — Template + mandatory-tag system (Pavle directive 2026-06-01)

**Directive:** Pavle: *"sve mora da ima neki šablon — kako piše skill, kako piše daily, kako piše wiki ... sve mora kad ima neki tag ... tako izbegavamo da se nešto smulja, da bude loše, da ne može da se pretražuje."*

**DECISION:**
1. **Every durable type with a markdown face has a canonical `Template`** (a governed, versioned durable object, like `domain_policy`): defines required frontmatter keys (envelope + type-specific), required body sections, and **required tag/category slots**.
2. **Mandatory tagging is an invariant (TAG-1):** no durable object persists without (a) a resolved `domain` (R3) and (b) ≥1 `category` from the governed taxonomy. An untagged write is **rejected to quarantine** by `ingest_guard`, not silently stored.
3. **`TemplateGate`** runs inside `ingest_guard` (or immediately after): a write to a templated type must satisfy its template's required fields/sections/tags, else quarantine + review. This is what stops "smuljano" content.
4. **Templates seed the renderers:** `altevra-vault` and the skill/hook renderers (§5) render *from* the template, so generated faces are structurally identical every time (composes with deterministic render T3).
5. **Categories stay a living taxonomy** (§1.4.9) — auto-created on capture, merges/renames review-gated — but **at least one is always required** (TAG-1). Domains stay governed (R3).

**WHY:** Templates + mandatory tags ARE the search substrate now that vectors are out (R12). Structure + governed tags = deterministic, reliable retrieval for both Pavle's agents and Altevra's resident modes. No structure → not findable.

**CODE IMPACT:** new `template` durable object + `migration` (seed builtin templates: skill, hook, wiki_page, daily_brief, decision, learning, person, preference, insight_card). `altevra-core::template` module + `TemplateGate`. Wired into `ingest_guard` (T1.8) and every renderer.

**P0 phase:** P0.0 (template schema + builtins for the P0.1 types) + P0.1 (TemplateGate enforced in the vertical loop) + per-phase as new faced types land.

---

## R14 — Modular core + small focused agents (Pavle directive — confirmation)

**Directive:** Pavle: *"arhitektura koja će biti modularna, da mogu da konektujem kasnije još stvari ... promptovi za agente u Altevri — nije jedan veliki agent, nego manji agent."*

**DECISION (confirms existing design, makes it a hard rule):**
1. **New integrations are modules, never core edits:** a new AI tool = a new `ToolAdapter` impl; a new capability = a new crate or registry entry. The 15-crate workspace + `ToolAdapter` trait + MCP adapter pattern is the extension surface. Core stays stable (MOD-1).
2. **Resident agents are small, single-purpose modes** (§4 `resident_mode`), each with its own narrow prompt and one job (observer, wiki_curator, daily_briefing, insight_synthesizer, …). **No monolithic mega-agent.** Each mode reads a scoped context packet and emits one kind of typed output (MOD-2).
3. **Prompt registry is layered + per-mode** (§4.8) — small composable prompt layers, not one giant system prompt.

**WHY:** Pavle's explicit design preference; also what makes the system maintainable for decades and testable (small agents = small, verifiable contracts).

**CODE IMPACT:** none new — confirms §4 + the adapter/crate structure. Guard: BUILD_TASKS must not introduce a monolithic agent; every resident capability is a mode.

---

## R15 — Opt-in hybrid semantic layer ABOVE the deterministic core (Pavle directive 2026-06-02)

**Directive:** after researching retrieval, Pavle chose a BGE-M3 hybrid (dense + lexical, RRF-fused). This refines — does NOT revoke — R12.

**DECISION:**
1. **Core retrieval stays exactly as R12 mandates:** tag/structured + FTS5 BM25 + graph, vector-free. Packet compiler, safety gates, leak-eval, packet determinism are unchanged. `R12-INV` still holds for the core path.
2. **A semantic layer is permitted as an OPT-IN ADDITION on top:** local **BGE-M3 dense** embeddings (`fastembed`, on-device) stored in **sqlite-vec** (single-binary; no separate service — Qdrant rejected to preserve local-first/single-binary), fused with existing FTS5 lexical results by **RRF** (`hybrid_rrf`, k=60). Dense + BM25 hybrid; BGE-M3 learned-sparse skipped (FTS5 covers lexical).
3. **Off by default:** `[llm] embedding_mode = "off"` (default) keeps the system identical to today. `"local"` activates it. The dense lane lives behind the `embedding` cargo feature (onnxruntime + sqlite-vec), so the default build/tests are byte-unchanged.
4. **SI-7 preserved for embeddings:** personal/high-water domains MUST embed locally. `DomainPolicyRepository::embedding_role_for` resolves the per-object role (R3 most-restrictive); `EmbeddingRouter` makes the cloud embedder structurally unreachable for `local_private`.

**WHY:** semantic recall compounds Altevra's value over years (CLAUDE.md §3.4) without sacrificing the deterministic, leak-safe core R12 bought. Gates/eval stay vector-free; relevance gets a dense boost when Pavle opts in.

**CODE IMPACT:** `altevra-memory`: `hybrid_rrf`, `embedding_router` (dep-free, always compiled); `bge`, `vec_store_sqlite` (feature `embedding`). `altevra-db`: `EmbeddingModelRole` + `embedding_role_for`. `[llm].embedding_mode` config. NO change to packet compiler / gated_packet / golden_eval.

**P0 phase:** opt-in add-on; default-off preserves all P0 exit criteria.

---

## Invariant addendum (new, from this reconciliation)

- **HP-1:** No MCP/agent caller has any approve/apply/grant/forget-execute/policy-set path. Enforced by absence in code.
- **HP-2:** `"approved"` is never an accepted input field; approval is core-recorded after a presence check.
- **R1-INV:** ceiling comparisons touch only `sensitivity_level` (6-ladder total order); domain and risk_tag are separate gate conditions.
- **R5-INV:** purging a `context_packet` body never touches its `exposure_decision` audit row.
- **R12-INV:** no code path uses vector/embedding similarity for P0 *core* retrieval; the core retrieval = tag/structured + FTS5 + graph only.
- **R15-INV:** the hybrid dense layer is opt-in (`embedding_mode=local`, feature `embedding`) and lives ABOVE the core; it never enters the packet compiler, gated_packet, or golden_eval. `local_private` content embeds locally only (cloud embedder structurally unreachable).
- **TAG-1:** no durable object persists without a resolved `domain` + ≥1 governed `category`; untagged → quarantine.
- **TEMPLATE-1:** a write to a templated type must satisfy its `Template` (required fields/sections/tags) or it is quarantined; renderers render *from* the template.
- **MOD-1:** new integrations are adapters/modules; core crates are not edited to add a tool/connector.
- **MOD-2:** every resident capability is a small single-purpose `resident_mode`; no monolithic agent.

---

## Sign-off

This file is the locked resolution of all cross-section contradictions and open questions. Pavle's acceptance turns the six deep sections from "drafts with conflicts" into a ratified contract set. Next artifact: `BUILD_TASKS.md` (granular execution plan mapped to the live code).

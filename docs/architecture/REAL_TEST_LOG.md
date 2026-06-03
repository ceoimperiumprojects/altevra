# Altevra Real Integration Test Log

> Live cross-tool tests. One entry per run.

## 2026-06-03 (Faza B) — upali pamet: baza misli preko codex GPT-5.5 ✅ PASS

Workflow-orchestrated (Foundation ∥ → Brain jobs → 3-lens verify incl. LIVE codex) + parent
full baseline. 801 → **816 tests / 0 fail**; clippy `--all-targets -D` clean (incl. embedding);
fmt. Branch altevra-overnight-p0.

- **B1 — ProposalsRepository + resident→proposals wiring** (`0a5780e`): dedup by `dedup_hash`
  (collision → increment evidence_count, never 2nd row); SI-9 tier always re-derived by core
  (agent can't assert); SI-14 — only a `Completed` run writes rows, `FailedSchema`→0.
- **B2 — 4 missing mode prompts** (`62b9fa2`): insight/observer/personal_curator/
  skill_factory_proposer in `06-skills/resident-agent-modes/`; all 8 modes now resolve a
  non-empty prompt; personal_curator pinned `local_private` + restricted (SI-7).
- **B3 — daily_briefing STUB → brain that notices** (`31d9c2e`): `detect_patterns()` + first
  caller of `last_contact()` ("haven't talked to Đorđe in N weeks" via new `dated_mentions`)
  + decisions past `review_after` ("still applies?"); codex prose synth when non-noop.
- **B4 — insight_synthesizer → recallable insight_card** (`2ea1530`): persists an InsightCard
  (auto-indexed via A1) instead of a 240-char log; noop-skips clean.
- **B5 — auto-categorization (living taxonomy)** (`cd5752f`): CheapWorker classify → tag or
  `kind="category"` Tier-0 proposal; SI-7 routes high-water → local_private.
- **Schema-landing fix** (`7f3b4ad`): runtime was sending only the one-line mode description,
  not the output contract → every insight/synthesis run was `failed_schema`. Added
  `GENERIC_OUTPUT_CONTRACT` to `run_dry` + a fence-tolerant balanced-brace JSON extractor in
  `parse_resident_output` (SI-14 still strict). All 8 mode prompts aligned to the generic
  `{proposals:[{kind,title,body,evidence_refs}]}` envelope.
- **SI-7 content fail-safe** (`7575cbd`): auto-categorizer no longer trusts `obj.domain` alone
  — a high-water content scan (`content_is_high_water`, reusing ingest_guard's keyword net)
  keeps a personal thought captured as generic `learning`/business OFF the cloud worker.

**LIVE codex_oauth (GPT-5.5) end-to-end on REAL `Decisions.md`** (read-only copy; vault sha256
`0e95c96…` identical before/after): token ALIVE (PONG). `resident run insight
--reasoning-mode codex_oauth` → **`status=completed`, 4 proposal rows** with real GPT-5.5
titles citing captured object ids ("Altevra direction converging from task tracker to thinking
OS", "GTM memory is the execution bridge", …) — the recorder now THINKS. 3-lens verify all
PASS: baseline/test, SI-7/leak/injection (high-water never cloud, SI-14 zero-on-invalid,
note-as-data), real-data live codex.

## 2026-06-03 (Faza A) — retrieval temelj: sve pretraživo + R5 audit ✅ PASS

Workflow-orchestrated (implement write-side → read-side, then 4-lens adversarial verify) +
parent full-baseline verify. Baseline 790 → **801 tests / 0 fail**; clippy
`--workspace --all-targets -D warnings` clean (incl. `--features embedding`); fmt. 9 local
commits, pushed to origin/altevra-overnight-p0.

- **A1 — all durable writers → guard+index** (`fc9da9f`): DecisionsRepository
  (`save_decision_indexed`) + WikiPagesRepository (`upsert_indexed`) + memory-lane
  (`guard_document`/`fts_index_chunk`) now route the redaction verdict → object_index +
  object_fts. Fail-closed: only `clean`/`redacted` index; unscanned/empty never enters
  (TAG-1). Wiki CLI `list --sync` wired to `upsert_indexed` (`e7557b2`) so synced pages
  are recallable (fail-closed on credential-class).
- **A2 — ExposureDecisionsRepository** (`268038b`): R5 content-free audit (counts +
  by_reason map; NO object ids/titles — §2.13 no-existence-leak). Emitted on every MCP
  packet compile. Migration left byte-identical (sqlx checksum on live DB); contract
  documented in repo source (`c10402e`).
- **A3 — PacketCompiler BM25+graph fusion** (`b9f6984`): relevance =
  `0.25*bm25 + 0.45*tag + 0.15*graph + 0.15*recency`. bm25 rank-normalized (byte-equal
  across SQLite versions), graph = mentions-edge overlap to FTS anchors. Compiler stays
  db-free (signals precomputed in `altevra-mcp::packet_build`); gates strictly before rank;
  id tie-break for determinism. Comment fixed to 0.45 (`4f6da82`).
- **A4 — CLI context ↔ MCP parity** (`d29cf17`): both surfaces call one
  `compile_gated_packet`; `context_packet_parity` drives BOTH real shapers and asserts a
  byte-equal packet (`85d6522`) so a future single-surface divergence fails.
- **Decision-label fix** (`9459f15`): atomized decisions recall as `decision`, not `learning`.

**LIVE on REAL `~/Obsidian/Imperium/Memory/Decisions.md`** (read-only copy; vault byte-identical,
sha256 verified before/after): 34 decisions atomized; `recall "validated under 20 numbers"` →
the real DECISION returned (pre-A1 invisible — only learnings were indexed). 4-lens adversarial
verify all PASS: baseline/test, SI-7/leak (planted secret grep-absent from the whole DB file),
determinism/R12, real-data recall.

## 2026-06-02 (session 9) — `recall_about` MCP tool: entity graph over MCP ✅ PASS

Made the mention graph UNIVERSAL — Claude Code / Cursor / Codex (not just the CLI)
can now ask "what about Đorđe". Baseline 783 → 790 tests, 0 fail; clippy
`--workspace --all-targets -D warnings` clean; fmt.

- **Loader moved to `altevra-vault::entity_dict`** (the small seam): the
  serde_yaml dictionary loader (People.md + projects.yaml + Projects/ dirs +
  mentor seed) + `resolve_entity` now live in `altevra-vault` (already a dep of
  both the CLI and `altevra-mcp`). The CLI `entity_dict` is a thin re-export — no
  duplicate logic, no new heavy dep. `add_person` now also tokenizes the name into
  per-token aliases (so `recall_about {entity:"Dimitrijević"}` resolves to Đorđe).
- **MCP tool `recall_about { entity, window?, limit?, db_path?, vault? }`** in
  `tools_sessions.rs` + wired into server.rs list/dispatch (gets `vault_path`).
  Resolves the name via the shared dictionary (diacritic/case/inflection-
  insensitive), returns `mentions`-linked objects recency-sorted with breadcrumbs.
- **Exposure gating (R11 #4) for OBJECTS**: new `object_exposable` builds an
  Envelope from the learning's real domain+sensitivity+redaction and runs the same
  `ExposureGate` as the turn reads — a Restricted (relationship/health/personal)
  note linked to an entity is WITHHELD, never leaked through the graph. Unknown
  name → clean not-found (nothing sensitive). 7 new tests incl. the high-water
  gate + clean-miss.
- **LIVE via the release `altevra serve` MCP server** (vault never written): seeded
  a temp DB by atomizing copies of real People.md (10 obj / 13 edges) +
  Decisions.md (31 / 22). `recall_about {entity:"Đorđe"}` → resolved person,
  **count 1** (the inflection-matched "Đorđetova direktiva" decision, breadcrumb
  `decision · business · 1w ago`); `{entity:"ReVesta"}` → project, **count 13**;
  `{entity:"Djordje"}` (ascii) → same entity; unknown name → clean miss. recall_about
  present in tools/list. Real vault untouched.

This closes the entity arc — the cross-link engine is now reachable by every AI
tool Pavle uses, with the same safety gate as every other read.

## 2026-06-02 (session 8) — entity extraction → mention graph ✅ PASS

Keyless (no LLM/NER) cross-link: when captured text mentions a known person/project,
link it. Answers "šta sam radio sa Đorđem" (vision §4.1) + seeds the "haven't talked
to X in N weeks" proactive query (§3.6). Baseline 764 → 783 tests, 0 fail; clippy
`--workspace --all-targets -D warnings` clean; fmt.

- **`altevra-core::entity` (pure, 11 tests)** — `EntityDictionary` built FROM the
  vault (People.md `##` headings → people, projects.yaml + Projects/ dirs →
  projects, + a mentor seed for Đorđe/Srđan/Saša who live in body text only).
  `detect_mentions` is word-boundary + ascii-fold (Đorđe = Djordje = Dimitrijević
  all hit one entity), longest-alias-wins, no substring false positives (`ss`
  never matches inside "assistant"). **Serbian inflection-tolerant** for person
  names (Đorđe→`Đorđetova`, Srđan→`Srđanu`, Đorđe→`Đorđem`) via a short-suffix
  whitelist — guarded so `Lukavac` never matches `Luka`. `last_contact` helper.
- **`altevra-db::MentionsRepository` (4 tests)** over the existing `relations`
  table (rel=`mentions`); idempotent edges (empty-string `to_ref` sentinel defeats
  SQLite's NULL-distinct), `clear_from_prefix` for re-atomize reconcile.
- **Wired into `capture --atomize`** (+`entity_dict` loader): each section links to
  the people/projects it mentions; re-atomize reconciles edges (dropped mention →
  edge removed). SI-7 unaffected (edges are local SQLite).
- **CLI `altevra recall --with <name>`** — resolves the name (diacritic/case/
  inflection-insensitive) and lists objects that mention it, recency-sorted with
  breadcrumbs.
- **LIVE on a READ-ONLY copy of real Memory/** (vault never written): atomized
  People.md (10) + Decisions.md (31) → **33 mention edges**. Graph summary:
  `project:revesta`=19 objects, imperium-crawl=3, hyper-pipeline=3, claw-network=2,
  + people luka/kim-eshan/stefan/ivan-kadic. `recall --with ReVesta` → 10 items;
  `recall --with Luka` → 2 items with breadcrumbs; **`recall --with Đorđe` → the
  decision mentioning "Đorđetova direktiva"** (inflection match — found a real
  cross-link a naive matcher would miss). `Djordje` (ascii) resolves to the same
  entity.

Left for Pavle / next: MCP `recall_about {entity}` tool (CLI `recall --with` covers
the use case now; the dictionary loader uses serde_yaml which lives in the CLI, so
exposing it via MCP needs a small loader move — deferred to keep scope tight).

## 2026-06-02 (session 7b) — `recall_window` MCP tool: recent memory by time, no query ✅ PASS

Small follow-on: a dedicated MCP tool to ask "what happened in the last week"
without a search term. Baseline 761 → 764 tests, 0 fail; clippy clean; fmt.

- `SessionsRepository::recent_turns_with_provenance(project, tool, since, until,
  limit)` — query-LESS recency listing (newest first) with the same provenance
  LEFT JOIN as the search variants. 1 unit test.
- `recall_window {window?, since?, until?, project?, tool?, limit?}` MCP tool —
  defaults to `last_week`, fail-closed on a bad window, same R11 #4 exposure gate
  as `search_turns` (recency never bypasses the ceiling). Wired into server.rs
  tool list + dispatch. 2 unit tests (lists-without-query, rejects-bad-window).
- **LIVE via the real release `altevra serve` MCP server**: `recall_window` present
  in tools/list; called with a seeded DB (2 turns) → `window: last_week, count: 2`,
  both turns newest-first with breadcrumbs `claude-code · altevra · just now`.
  isError:false. Exactly the "what happened lately" use case, no query needed.

## 2026-06-02 (session 7) — `capture --watch`: auto-atomize living docs on save ✅ PASS

Closed the atomization loop — no more manual `altevra capture <file>`. A watcher
auto-atomizes living docs whenever they're saved, incrementally + idempotently.
Baseline 755 → 761 tests, 0 fail; clippy `--workspace --all-targets -D warnings`
clean; fmt.

- **Incremental re-atomize (`atomize_file`)** — reconciles each file's prior objects
  (same `capture-<filestem>-` id prefix). Edit a section → new content hash → new id
  + stale id `forget`-ten; delete a section → forgotten; add a section → new object;
  unchanged section → same id (idempotent). `LearningsRepository::insert` is now
  `INSERT OR REPLACE` so re-writing an unchanged id never UNIQUE-violates;
  `ObjectIndexRepository::ids_with_prefix` finds the file's prior objects (LIKE-escaped).
- **`altevra capture --watch [--path …] [--debounce-ms N]`** — async watcher mirroring
  `WatcherDaemon` (notify → tokio mpsc → `Debouncer` → `tokio::select!`, Ctrl+C
  shutdown). Initial atomize pass, then blocks watching. One-shot `capture <file>`
  unchanged (`file` now Optional, required unless `--watch`).
- **SI-7 held** — watcher writes SQLite ONLY; never the vault. `atomize_file` still
  infers domain + escalates high-water (People→relationship→Restricted).
- **Headline unit test** `incremental_reatomize_reflects_exactly_v2`: v1 (3 objects)
  → v2 = exactly {1 unchanged-by-id, 1 updated, 1 removed→forgotten, 1 new}; 3 live
  objects, no dupes; recall finds new text, not old. Plus a LIVE `watch_until_shutdown`
  loop test (create file while watching → object lands → recall confirms).
- **LIVE on a READ-ONLY COPY of real `~/Obsidian/Imperium/Memory/`** (vault never
  written): initial pass atomized **41 objects** (31 Decisions + 10 People). Appended
  a section live → watcher → **42 objects**, recall found it. Edited that section's
  text → still **42** (updated, NOT duplicated), **1 forgotten** (stale), recall:
  NEW text = 1 hit, OLD text = "No memory of…". Watcher log shows
  `↻ Decisions.md: 32 captured, 1 forgotten`. Real vault untouched; temp DB + copy
  deleted after.

## 2026-06-02 (session 6) — Section templates: per-`##`-section conformance + LLM rewrite seam ✅ PASS

Pavle's follow-up: *"svaki dokument prati šablon ČAK I DELOVE u dokumentu … sve
lepo da se piše sve"* — every `## ` section must conform to a per-type contract,
not just the document frontmatter. Built in two phases; baseline 749 → 755 tests,
0 fail; clippy `--workspace --all-targets -D warnings` clean (incl. `--features
embedding`).

**Phase 1 — keyless section-template conformance (fully done):**
- `altevra-vault::section_template` — per-type `SectionContract` with SYNONYM SETS,
  **calibrated against the real `Memory/*.md`** (decision: `Odluka` + `Zašto`/`Šta
  znači`/`Razlog`/`Why`/…; person: `Kontekst` + `Uloga`/`Status`/…; learning/daily/
  note = freeform, since real `Learnings.md` is 12/16 plain prose — labels never
  forced). Matches block-level AND list-item (`- **X:**`) labels. 18 unit tests.
- `vault normalize` DRY-RUN now reports section conformance + splits non-conformant
  into `scaffoldable` (empty/stub) vs `need_rewrite` (prose missing labels).
  `--scaffold-empty` fills ONLY empty/stub sections (backup-first, idempotent, never
  touches prose). `capture --atomize` tags each object `conformant`/`needs-structure`.
- **REAL DRY-RUN ~/Obsidian/Imperium (read-only):** 3891 sections; **27
  non-conformant across 2 strict-type files** (Decisions 21/31 missing `Zašto`/
  `Odluka`, People 6/10 missing `Uloga`/`Kontekst`); 0 scaffoldable, 27 need_rewrite
  (all prose). **REAL atomize of Decisions.md:** 10/31 conformant, 21 needs-structure
  — calibration confirmed correct (the `Odluka`+`Šta znači` sections pass; a
  `**Math:**`-style section is genuinely flagged, not a false positive).

**Phase 2 — LLM rewrite seam (wired + noop-tested; NOT run live on the vault):**
- `altevra-vault::build_rewrite_prompt(section, type) -> RewritePrompt` — pure,
  deterministic; system prompt makes "preserve EVERY fact (output MUST contain all
  input facts)" a hard contract; model only reorganizes under the labels. 3 tests.
- `vault normalize --rewrite` routes `altevra_llm::build_router`. DRY-RUN by default;
  needs `--apply` + a real provider to write; backup-first.
- **REAL --rewrite DRY-RUN ~/Obsidian/Imperium (delegated default):** `provider=noop
  (noop:true)`, `sections_need_rewrite=27`, `would_rewrite=27`, `rewritten=0`,
  `backup=None` — vault UNCHANGED (frontmatter count still 23). The no-op seam is
  live-verified with NO model call. Honest status: a live LLM rewrite needs
  `reasoning_mode = codex_oauth` (ChatGPT Plus, no key) or `api` + `--apply`; that
  real run is **left to Pavle** (per the hard rule: no live LLM rewrites on the real
  vault from here).

## 2026-06-02 (session 5) — Atomizacija: section atomize + vault normalize, live on REAL vault ✅ PASS

Pavle's "Atomizacija" directive — the human writes few files, the machine sees
many atomic objects. Built on the existing capture/recall + frontmatter/template
substrate; baseline stayed green (697 → 728 tests, 0 fail; clippy
`--workspace --all-targets -D warnings` clean, incl. `--features embedding`).

- **`altevra-vault::sections` (pure parser) ✅** — `parse_sections` splits a living
  aggregate into its `## ` sections (preamble dropped, `###` stays nested,
  empty-body skipped, `YYYY-MM-DD` heading → date). 13 unit tests.
- **`altevra capture --atomize` LIVE on REAL data ✅** — read-only copy of the real
  `~/Obsidian/Imperium/Memory/Decisions.md` atomized into a temp DB:
  `kind=decision domain=business sections_found=31 captured=31 skipped_credential=0`.
  `altevra recall "direct-call hypothesis validated"` → **exactly 1 hit** (the one
  section), `recall "ICP weighting attorneys"` → **exactly 1 hit** — individual
  sections recallable, not the whole file. Real `People.md` → 10 `person`-kind
  objects, domain auto-inferred `relationship`, sensitivity escalated `restricted`
  (high-water). Integration test proves a fake `sk-live…` (concat!) key is redacted
  in every stored section body; a credential-class (`rejected`) section is skipped,
  others still captured.
- **`altevra vault normalize` DRY-RUN on REAL `~/Obsidian/Imperium` ✅** — read-only:
  **512 md scanned, all 512 would get/merge frontmatter, 0 already normalized, 9
  excluded (Templates/), 2 skipped (invalid-UTF-8 hook dumps — never corrupted)**.
  by_type: note 243, daily_brief 86, content 76, idea 65, reference 21, wiki_page
  17, decision/learning/person/research 1 each. Vault UNCHANGED (pre-existing
  frontmatter count 23 → still 23). 3 before/after sample diffs printed (Archive/
  Daily → `type: daily_brief, status: archived`).
- **`--apply` proven on a COPY of the real vault ✅** (real vault untouched): backup
  written to `obsidian-normalize-<ts>/` holding the ORIGINAL content FIRST, then
  512 files merged (frontmatter prepended, body verbatim incl. `##` markers),
  idempotent re-apply wrote 0. The real `--apply` against `~/Obsidian` is left to
  Pavle (DRY-RUN only here per the safety rule).
- **Spec:** `docs/architecture/VAULT_DOCUMENT_TEMPLATE.md` (frontmatter contract +
  folder map + atomization rule, refs R13/R12/R3/R1).

## 2026-06-02 (session 3) — LLM provider modes + hybrid lane, live-verified ✅ PASS

LLM provider work (plan `giggly-humming-ullman.md`, R15). All live, real-world:

- **codex_oauth reasoning → GPT-5.5 LIVE ✅** — `CodexOAuthProvider::from_default_auth()` against the REAL `~/.codex/auth.json`; minimal request returned `"PONG"`. Reverse-engineered the ChatGPT codex Responses contract through its 400s: instructions required, `store:false`, `stream:true`+SSE, no max_output_tokens. Auth accepted (not 401) → token valid. **This mode works today with NO API key**, on the existing ChatGPT Plus seat. (test `live_codex_completes`, `#[ignore]`.)
- **sqlite-vec dense store LIVE ✅** — `--features embedding`: `SqliteVecStore::open_in_memory` (statically-linked extension) → upsert 3 vecs → KNN returns nearest in correct order. Single-binary, local (SI-7). (tests `upsert_then_knn_roundtrip`, `dim_mismatch_errors`.)
- **config CLI flow ✅** — real debug binary: `config set/get/show llm.{reasoning_mode,embedding_mode,codex_model}` round-trips; `[llm]` persists to `.altevra/config.toml` (correct TOML table ordering); invalid mode rejected with clear message.
- **MCP connection re-smoke (post-changes) ✅** — `scripts/p0_mcp_smoke.sh` (debug bin): initialize → tools/list (delegation tools present: get_resident_prompt, get_context_packet, build_system_prompt, list_resident_modes) → get_capabilities (isError:false). Provider/embedding changes did NOT break the connection. Confirms `delegated` mode: a connected Claude/Cursor pulls prompt+packet and writes back via save_*.
- **Baseline ✅** — 674 workspace tests pass / 0 fail; `cargo clippy --workspace -D warnings` clean; `--features embedding` clippy clean; onnxruntime/fastembed compile.

Awaits Pavle (honest): `api` mode keys (`altevra secrets set <KEY>`); BGE-M3 real inference (first run downloads ~2GB model — `bge_embeds_with_correct_dim` `#[ignore]` ready); interactive Cursor TUI spawn in a herdr pane (his hands-on; CLI+config verified ready).

## 2026-06-02 (session 4) — temporal recall + skill cross-tool inventory ✅ PASS

Two follow-on features built and live-verified after Pavle's product feedback:

- **Temporal recall LIVE ✅** — answers his exact use case "šta smo radili pre mesec dana sa Amerikancima". `altevra-core::time_window` parses `24h`/`7d`/`30d`/`3mo` + presets `last_week`/`last_month`/etc (fail-closed on garbage — typo never widens a window). `SessionsRepository::search_turns_in_window` added; existing `search_turns` delegates with `None,None` (back-compat preserved across 12+ call sites). MCP `search_turns` tool now accepts `window`/`since`/`until` params. **Live MCP smoke**: `window="last_month"` → correct 30d range echoed; `window="garbage"` → fail-closed error listing valid forms; `since="2026-05-01" until="2026-05-31"` → date-only parsing works. Headline DB test (3 turns: month-old/yesterday/unrelated about "Americans"; window 25-40d back) returns ONLY the month-old Americans turn.
- **Skill cross-tool inventory LIVE ✅** — answers his use case "kad ubacim skill u Claude da se automatski vidi u svima". `altevra-skills::importer` scans `~/.{claude,codex,cursor,hermes,imperium}/skills/` with a loose YAML parser tolerating Claude/Hermes `name:` AND Altevra `slug:`/`title:` (separate from the strict `parse_skill` — authoring contract untouched). Detects `<!-- ALTEVRA_MANAGED -->` marker. `altevra skill inventory` CLI command — read-only first pass before any propagation. **Live on real disk**: 137 unique skill slugs across 5 tools. `--missing` flag exposes propagation candidates (e.g. `audit` is in claude+codex+cursor but missing from hermes+imperium). No writes performed (sync `--apply` is the next increment).

Baseline 685 tests pass / 0 fail; clippy `--workspace -D warnings` clean. 15 commits on `altevra-overnight-p0`.

## 2026-06-02 (session 4 cont'd) — skill sync propagation engine ✅ DRY-RUN PASS

Built the sync engine on top of the inventory; explicitly stopped at apply (Pavle authorization).

- **Sync planner DRY-RUN LIVE ✅** — on real disk (137 unique slugs across 5 tools): `326 creates planned, 359 skips, 0 refreshes` (refreshes = 0 because nothing was ever synced before). Skips correctly classified UserAuthored for every third-party skill in claude/codex/cursor — those NEVER get touched. Filter `--slug altevra --to hermes` correctly isolated 2 propagation candidates (altevra-core + altevra-agent-operations → hermes).
- **Hard safety invariants enforced**: NEVER overwrites a non-`ALTEVRA_MANAGED` file; atomic write (write-temp + rename) so a crash mid-write leaves no half-file; managed header injected so subsequent syncs are idempotent (`AlreadyInSync` skip when content matches).
- **3 sync unit tests** cover the full lifecycle: create-vs-skip (UserAuthored), apply-with-header-and-refresh-on-drift, source-preference (user-authored > managed).
- **APPLY ✅ EXECUTED LIVE after Pavle's explicit go** ("smeš slobodno, brati") in two stages:
  1. **Smallest safe (2 files → hermes)**: created `~/.hermes/skills/altevra-core/SKILL.md` + `altevra-agent-operations/SKILL.md`, managed header verified, idempotent re-run returned 2× `AlreadyInSync`.
  2. **Full sync (324 files across 5 tools)**: `created: 324, refreshed: 0, skipped: 361, errors: 0`. Post-sync inventory confirms every skill is now in `[claude,codex,cursor,hermes,imperium]` with `(managed)` flag. **Live cross-tool effect confirmed** — Hermes-only skills (`dogfood`, `yuanbao`, `hp-arch`, `hp-research`) now appear in Claude Code's session-start skill list.

Baseline 688 tests pass / 0 fail; clippy `--workspace -D warnings` clean.

## 2026-06-02 (session 4 cont'd) — real-time skill watcher ✅ END-TO-END PASS

Final piece of "AUTOMATSKI da se prebacuje" — the watcher daemon. Started after Pavle's "ajde nastavi" follow-up.

- **`altevra-skills::watcher`** module on top of `notify` (already used by `altevra-watcher`). `watch_loop` runs a long-running mpsc loop over CREATE/MODIFY/REMOVE events; debounces 2s; per-cycle re-plan + (optional) re-apply. Filters out our own `.altevra-tmp` write-temps and editor `.swp` to prevent re-trigger loops.
- **`altevra skill sync --watch [--apply]`** CLI flag — initial sync, then blocks watching. Ctrl+C is observed via tokio signal + std::mpsc stop channel.
- **LIVE END-TO-END VERIFIED** in two runs:
  1. **DRY-RUN watch**: created `~/.hermes/skills/altevra-watch-test/SKILL.md` at runtime → cycle log "↻ cycle: triggers=…altevra-watch-test/SKILL.md | planned creates=4 refreshes=0 skips=686". Zero writes (dry-run).
  2. **APPLY watch (Pavle's explicit go from previous step)**: same injection → all 4 other tools (`claude`, `codex`, `cursor`, `imperium`) received the skill with managed header within debounce window. Each file's first line: `<!-- ALTEVRA_MANAGED: true -->`. Cycle log: `applied creates=4 refreshes=0 skips=686`. **Direct system-level confirmation** — the injected test skill briefly appeared in Claude Code's SessionStart skill listing during the run.
- Test artifacts cleaned up after each run (no test skill left in tool dirs).

Baseline 690 tests pass / 0 fail; clippy `--workspace -D warnings` clean. Pavle's "AUTOMATSKI da se prebacuje, brate" — now real.

## 2026-06-01 — MCP stdio connection (autonomous half) ✅ PASS

**What:** Built the real release binary (`cargo build --release -p altevra-cli`,
exit 0) and drove the actual `altevra serve` MCP server over stdio with real
JSON-RPC 2.0 — exactly how Claude Code / Cursor / Codex talk to Altevra.

**Script:** `scripts/p0_mcp_smoke.sh`

**Results (verbatim from the live server):**
- `initialize` → `{"result":{"protocolVersion":"2024-11-05","serverInfo":{"name":"altevra","version":"0.3.0"}}}` ✅
- `tools/list` → 36 tools returned with full input schemas, incl.
  `get_agent_bootstrap_packet`, `get_context_packet`, `search_memory`,
  `get_source_of_truth`, `create_review_item`, `get_capabilities` ✅
- `tools/call get_capabilities` →
  `{"adapters":["claude-code","codex","cursor","antigravity"],"hooks":["session_start","session_end","on_error"],"mcp_tools":22,"skills":[]}` ✅

**Verdict:** The MCP connection works end-to-end against the real binary. This is
the autonomous, deterministic half of the "real test" — proves any MCP client
(Claude Code, Cursor) can initialize, discover tools, and call them.

## 2026-06-01 — adapter connect dry-run (all tools) ✅ PASS

`altevra connect --tool <t> --dry-run` against the real binary:
- **claude-code** → detected; would update `.claude/settings.json` (MCP config) + instructions + 2 skills. ✅
- **cursor** → would create `.cursor/mcp.json` + `.cursor/hooks.json` + `.cursor/rules/altevra.mdc`. ✅
- **codex** → would create `.codex/config.toml`. ✅
- **drift safety (T4):** `AGENTS.md` flagged drifted, "won't overwrite without --force" for all three. ✅

**Verdict:** Altevra connects to Claude Code, Cursor, Codex with correct per-tool
config + drift protection. With the MCP stdio pass above, the connection layer is
proven against the real binary for every adapter.

## 2026-06-01 — LIVE herdr Claude agent spawn ✅ PASS (found + fixed 3 real bugs)

**What:** Spawned real `claude` agents in herdr (`herdr agent start … -- claude`,
split panes), pointed at Altevra, and told them to call the `get_capabilities`
MCP tool. This is the interactive half — and it did exactly what a live test is
for: **it surfaced 3 real integration bugs that the protocol smoke could not.**

**Live findings → fixes:**
1. **`altevra: command not found`** (hooks failed) → the binary wasn't on PATH.
   FIX: symlink `~/.local/bin/altevra → target/release/altevra`. ✅
2. **`mcp__altevra__*` not registered** (agent: "no altevra MCP server in this
   session") → project `.mcp.json` isn't auto-trusted. FIX: registered Altevra
   MCP at **user scope** via `claude mcp add altevra --scope user -- altevra serve`.
   Next agent then reported **"Called altevra — get_capabilities executed without
   error"** ✅ (connection works).
3. **Empty tool result** (agent: "executed but returned empty") → Altevra's
   `tools/call` returned BARE JSON, but Claude Code expects the MCP
   `{"content":[{"type":"text",...}]}` envelope. FIX: wrap every tools/call result
   in `content` + `structuredContent` in `McpServer::handle`. Verified via stdio:
   result now `{"content":[{"text":"{adapters:[claude-code,codex,cursor,antigravity]…}"}],"isError":false}`. ✅

**Verdict:** Live herdr spawn done; the Altevra↔Claude-Code connection is now
fixed end-to-end (PATH + MCP registration + response format). The smoke + adapter
proofs above plus these live fixes mean the connection layer is genuinely working,
not just protocol-shaped.

**Note:** Cursor live spawn not separately driven, but it shares the same MCP
server + the (now-fixed) response format, and `connect --tool cursor` config is
verified above.

## 2026-06-01 (session 2) — MCP live smoke after R11 + P0.5 + P0.6
- `scripts/p0_mcp_smoke.sh` against release binary: **PASS**.
  - initialize → serverInfo altevra v0.3.0 ✓
  - tools/list → all tools present incl. gated `replay_session` / `search_turns` (R11 J) ✓
  - tools/call `get_capabilities` → proper MCP content envelope, isError=false ✓
- Confirms the R11 turn-read gating + P0.5/P0.6 additions did NOT break the live MCP connection.
- `altevra resident run <mode>` live-verified separately (P0.5): personal_curator→local_private→noop(local), recorded as resident_run.
- Pending interactive half: Cursor spawn in herdr (Claude side previously live-tested).

## 2026-06-01 (session 2) — Cursor connection validated (autonomous half)
- `altevra connect --tool cursor` generates AGENTS.md + .cursor/mcp.json + hooks.json
  + rules/altevra.mdc. mcp.json registers server `altevra` → command `altevra serve`.
- Smoke through the EXACT spawn command cursor's config uses: initialize OK
  (altevra 0.3.0) + tools/list OK (36 tools incl get_context_packet). **Cursor would
  connect correctly.**
- Interactive half (a live Cursor TUI agent in a herdr pane calling Altevra) is
  Pavle's hands-on step — cursor CLI + herdr are both installed and ready.

## 2026-06-02 (session 5 cont'd) — LIVE Codex reformat on the REAL vault ✅ APPLIED

After Pavle's "preformuliši sve, ne menjaj sadržaj" + explicit "Da, pusti 21 sad":

- **Frontmatter apply (513 files) ✅** — real `~/Obsidian/Imperium` normalized; backup
  `obsidian-normalize-<ts>/` first; bodies verbatim (Decisions.md `##` count 31→31).
  Idempotency bug found+fixed live (`updated` was bumped each pass → rewrote all 513;
  now seeded-once → re-run reports "513 already normalized; 0 changes").
- **SI-7 guard added to --rewrite ✅** — high-water domain + non-local provider → skip.
  Live: codex_oauth (cloud) → would_rewrite=21 (Decisions/business), si7_skipped=6
  (People.md/relationship). People NEVER sent to ChatGPT.
- **LIVE Codex reformat of 21 Decisions sections ✅** — `--rewrite --reasoning-mode
  codex_oauth --apply`: rewritten=21, si7_skipped=6, backup `obsidian-rewrite-<ts>/`.
  Verified: Decisions.md section count preserved 31→31; conformance now 0
  non-conformant in Decisions (was 21); **People.md byte-identical to backup**
  (SI-7 held). Demo confirmed fact-preservation: a bullet-list section became
  **Odluka:**/**Zašto:**/**Pravilo za primenu:** with every fact retained.

Left for Pavle: the 6 People.md sections need a LOCAL model (SI-7 bars cloud) —
pending Ollama/vLLM, or hand-edit. Code all pushed; vault edits live on disk (+ backups).

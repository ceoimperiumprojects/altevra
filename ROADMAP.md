# Altevra — Roadmap

**Owner:** Pavle Anđelković
**Last updated:** 2026-05-28
**Current state:** v0.3.8 shipped (commit `de74351`), repo public,
436+35=471 tests green, MCP server live in Claude Code
**Companion docs:** [VISION.md](./VISION.md) (why),
[CLAUDE.md](./CLAUDE.md) (operating doctrine),
[ALTEVRA_NEXT_ARCHITECTURE_RESIDENT_AGENT_WIKI_PERSONAL_BRAIN.md](./ALTEVRA_NEXT_ARCHITECTURE_RESIDENT_AGENT_WIKI_PERSONAL_BRAIN.md)
(next-phase technical architecture)

---

## Roadmap principle

**Build the living loop first.** Altevra must become useful daily before
it becomes huge.

Pavle's directive (2026-05-28):

> *"Mislim da trebaju prvo da se odrade svi fiksovi da se sve sredi da
> bude kako treba, onda idemo dalje na sledeće faze, pa onda system
> promptovi za agente i ono što sam ti rekao malo pre da sve bude kako
> treba da Altevra bude živa."*

Order: **stabilize → foundation → resident → wiki → personal → daily
loop → self-improvement → skill generation → identity split → polish**.

---

## Phase 0 — Stabilization & Fix-ups (NOW)

**Goal:** Fix every broken thing so the existing v0.3.8 surface is rock
solid before we add new layers.

**Timeline:** This session.

### Fixes in queue

- [x] **Hook PATH issue** — symlink `~/.local/bin/altevra` → release
      binary (done; verified `altevra --version` from minimal env)
- [x] **Migration 17 mismatch** — release binary rebuilt with embedded
      migration 17; dev DB cleared and re-created cleanly (done; verified)
- [ ] **Antigravity parser noise** — turn the per-line WARN logs into
      a single summary line when timestamp/text format drifts (real
      antigravity files generate spam, not errors). Add `--quiet` flag
      on `altevra setup analyze-everything` for clean JSON output even
      with stderr noise from parsers.
- [ ] **Codex parser zero-turn sessions** — Codex's `history.jsonl` on
      Pavle's box yields 3 sessions / 3 turns (one-line histories).
      Verify whether real conversation content lives in `state_5.sqlite`
      tables we haven't tapped (`messages`?, `conversation_logs`?) and
      pull it in if so.
- [ ] **Cursor empty sessions** — current parser handles VS Code
      Cursor-extension chatSessions; real Cursor CLI uses
      `~/.cursor/ai-tracking/ai-code-tracking.db` (50,879 ai_code_hashes
      already on Pavle's box). Plan a proper `cursor_cli.rs` parser
      (see Phase 8).
- [ ] **Dead-code warnings** — 8 clippy warnings remain (`sort_by` →
      `sort_by_key`, unused `discover` helpers in parsers, etc.). Clean
      up so `cargo clippy --workspace -- -D warnings` passes.
- [ ] **Auto-install symlink on `altevra init`** — first-run wizard
      should symlink the release binary into `~/.local/bin/` if not
      already present, so hooks work out of the box.
- [ ] **`.altevra/.altevra.db` location** — consider migrating local
      dev DB from `<repo>/.altevra/altevra.db` to `~/.altevra/altevra.db`
      so each repo doesn't pollute its own state. (Or document
      explicitly that per-repo state is by design.)

**Exit criteria:** All known v0.3.8 paper cuts are resolved. `cargo test
--workspace` passes ≥471. Hooks work cleanly. Real import on Pavle's
laptop produces zero unnecessary stderr noise.

---

## Phase 1 — Resident Agent + Wiki Foundation (NEXT)

**Goal:** Ship the **prompt + schema foundation** for the Resident
Agent layer and Wiki Layer described in
`ALTEVRA_NEXT_ARCHITECTURE_RESIDENT_AGENT_WIKI_PERSONAL_BRAIN.md` §20
Phase 1 + Phase 2.

**Estimated:** 3-5h. Zero LLM dependency — pure markdown + JSON +
small CLI.

### Deliverables

1. **Mode prompts:** `06-skills/resident-agent-core.md` +
   `06-skills/resident-agent-modes/{memory-curator,synthesis,daily-briefing,wiki-curator}.md`
2. **System rules:** `00-system/{token-economy,protected-memory-rules,daily-capture-protocol}.md`
3. **Output schemas:** `00-system/schemas/{context_packet_v1,synthesis_v1,memory_curator_v1,wiki_curator_v1,daily_briefing_v1}.json`
4. **Wiki skeleton:** `wiki/{concepts,projects,people,patterns,decisions,domains}/` with three example pages:
   - `wiki/concepts/resident-agent.md`
   - `wiki/concepts/context-engineering.md`
   - `wiki/projects/altevra.md`
5. **Wiki parser:** `crates/altevra-vault/src/wiki.rs` — frontmatter +
   body + `[[wiki-link]]` extraction
6. **`wiki_pages` SQLite table:** migration 018, plus
   `wiki_page_links` graph table
7. **CLI:**
   - `altevra wiki list [--json]`
   - `altevra wiki show <topic>`
   - `altevra wiki search <query>`
   - `altevra resident modes [--json]`
   - `altevra resident prompt --mode <mode>`
8. **MCP tools:** `get_wiki_page`, `search_wiki`,
   `list_resident_modes`, `get_resident_prompt`
9. **Tests:** 12+ new (mode prompt loading, wiki page parsing, schema
   serialization, protected memory rules file existence)

**Exit criteria:** `altevra wiki list` returns the 3 seeded pages.
`altevra resident modes` lists 4 modes. Schemas parse via `serde_json`.

---

## Phase 2 — v0.3.9 Multi-Provider LLM (chat + embedding)

**Goal:** Generalize `altevra-llm` from Gemini-only into a true
multi-provider abstraction so subsequent phases can route by **role**
(cheap_worker, strong_reasoner, local_private, embedding, reranker)
instead of by hardcoded provider.

**Estimated:** 5-7h.

### Deliverables (from existing plan + role refinement)

1. `ChatProvider` + `EmbeddingProvider` traits (`crates/altevra-llm/src/traits.rs`)
2. Native adapters: Gemini, OpenAI, Anthropic, Voyage
3. OpenAI-compat universal adapter for: DeepSeek 🇨🇳, Qwen 🇨🇳, Moonshot 🇨🇳,
   Zhipu GLM 🇨🇳, MiniMax 🇨🇳, Baichuan 🇨🇳, Yi 🇨🇳, Stepfun 🇨🇳, Groq, Together,
   OpenRouter, Mistral, Cohere, Ollama (local), vLLM (self-hosted)
4. **Role routing** (NEW from 2026-05-28 architecture doc): config maps
   roles → providers; jobs request a role, get the configured provider
5. `~/.altevra/llm.yaml` config schema with providers + defaults + roles + per-job overrides
6. Refactor `altevra-memory::GeminiEmbedder` → wrap `EmbeddingProvider` (backward-compat shim)
7. Wire 3 placeholder LLM call sites (insight_synthesizer, leverage
   distillation, research synthesize) to use role-based routing
8. CLI: `altevra llm providers`, `altevra llm test <provider>`,
   `altevra llm route <job>`, `altevra llm models <provider>`
9. MCP tools: `llm_chat`, `llm_embed`, `llm_routes`

**Exit criteria:** Pavle can configure 3+ providers (Gemini, DeepSeek,
local Ollama), run `altevra llm test gemini` + `altevra llm test deepseek`
+ `altevra llm test ollama` and all return 200 OK in <2s. Brain
insight_synthesizer no longer returns "no LLM configured" stub.

---

## Phase 3 — v0.3.10 Onboarding + Docs

**Goal:** Polish UX so the project is *installable* from a fresh clone
without spelunking through Rust code.

**Estimated:** 2-3h.

### Deliverables

1. `altevra setup llm` — interactive wizard (detect installed providers,
   prompt API keys, test calls, save to keyring)
2. `altevra setup` no-args → interactive menu (tools, llm, vault, brain)
3. `altevra setup --quickstart` → full guided setup (init → connect
   tools → setup llm → optional analyze-everything)
4. Doctor LLM checks (per provider: ping `/v1/models`, verify embed dim
   matches stored vectors)
5. **Auto-install symlink** on `altevra init` (carry-over from Phase 0)
6. Docs:
   - README "Getting Started" sekcija revised
   - `docs/LLM_PROVIDERS.md` (full matrix + endpoints + sample config)
   - `docs/CHINESE_PROVIDERS.md` (Qwen/DeepSeek/Moonshot/Zhipu setup)
   - `docs/ANALYZE_EVERYTHING.md` (how-to + idempotency notes)
   - `docs/HOOKS.md` (PATH requirement + adapter list + event names)

**Exit criteria:** Fresh clone → `cargo build --release` → `altevra
setup --quickstart` → working install with hooks active in <10 min.

---

## Phase 4 — Resident Agent Runtime (Phase 3 of architecture doc)

**Goal:** Make the Resident Agent **executable**. Modes route to roles,
runs are logged, output schemas are enforced.

**Estimated:** 6-8h.

**Gated on:** Phase 2 (multi-provider LLM must exist for routing).

### Deliverables

1. `altevra-resident` crate (or module in `altevra-brain`) — new crate
2. `ResidentMode` enum, `ResidentRun` execution flow
3. Context Packet generator (`crates/altevra-context/`) — builds strict
   packets per mode from current Altevra state
4. Schema validation (`serde_json::Value` vs spec — fail fast on output
   drift)
5. `resident_agent_runs` SQLite table (migration 019) — logs every run
   with mode, role, model, tokens, cost, result
6. CLI: `altevra resident run --mode <mode> [--project] [--task] [--dry-run]`
7. MCP tools: `run_resident_agent`, `get_resident_modes`,
   `get_resident_prompt`, `get_context_packet`
8. First mode wired end-to-end: **memory_curator** (safest — read-only
   proposals, no destructive writes)

**Exit criteria:** `altevra resident run --mode memory_curator` reads
current memory, returns structured JSON with dedupe_suggestions, stale_items,
conflicts, category_suggestions, review_items. No writes happen without
explicit Pavle approval flag.

---

## Phase 5 — Wiki Curator + Auto-Wiki Pipeline (Phase 4 of architecture doc)

**Goal:** Living wiki that updates itself from new evidence.

**Estimated:** 6-8h.

**Gated on:** Phase 4 (resident runtime).

### Deliverables

1. **Wiki Curator mode** wired to resident runtime
2. Topic classifier — given a new event/session/research item, propose
   candidate wiki topics
3. Diff/review behavior — auto-apply low-risk updates, queue review items
   for sensitive (person/identity/decision) pages
4. CLI: `altevra wiki suggest --since 24h`, `altevra wiki synthesize
   --topic X`, `altevra wiki review` (interactive approve/reject queue)
5. New brain job: `wiki_curator_sweep` (4h period) — picks up recent
   items, runs curator, files diffs
6. `wiki_review_queue` SQLite table (migration 020)
7. Auto-link expansion (`[[topic]]` becomes graph edge in `wiki_page_links`)

**Exit criteria:** Brain auto-creates `wiki/concepts/<topic>.md` for
material topics after 24h of usage. Sensitive topics never auto-apply
without review.

---

## Phase 6 — Personal Brain Layer (Phase 5 of architecture doc)

**Goal:** Personal data first-class. Note types, identity seeds, sensitivity
labels, relevance gate.

**Estimated:** 8-10h.

**Gated on:** Phase 4 (resident) + Phase 5 (wiki) for surface
integration.

### Deliverables

1. New crate `crates/altevra-personal/` (or module in altevra-brain)
2. **Note types** — `Decision, Learning, Preference, Person, Relationship,
   Place, Idea, Goal, Mood, Health, Memory, Reference, Habit, Routine,
   Value, IdentityShift, LifeEvent` — polymorphic via `kind` column
3. Migration 021 — `personal_memory` table with sensitivity + review_required
4. **Identity seed split** (per 2026-05-28 directive):
   - **Hermes turf:** `~/.imperium/identity/profile.yaml` stays light
   - **Altevra deep:** `~/.altevra/identity/{profile,preferences,life-domains,active-goals,evolution}.yaml`
     — versioned, sensitive-labeled
   - MCP tool `altevra_identity_query(field, depth)` — Hermes calls
     when depth needed
5. **Relevance Gate** (`crates/altevra-research/src/relevance_gate.rs`):
   - `~/.altevra/interests.yaml` config — opt-in categories per life domain
   - Pre-fetch gate for research jobs (replaces blanket BM25)
   - `relevance_decisions` audit table (migration 022)
6. CLI: `altevra note add <kind> "..."`, `altevra note list`,
   `altevra interests {list,add,remove}`, `altevra identity query <field>`
7. MCP tools: `record_personal_note`, `query_personal`, `update_interest`,
   `altevra_identity_query`

**Exit criteria:** Pavle can `altevra note add preference "wakes up best
between 9-10am"`, `altevra note add person "Srđan Jovanović — VP People @ HTEC, mentor"`. Research jobs auto-filter content via relevance gate.
Hermes can query Altevra for identity depth via MCP.

---

## Phase 7 — Daily Briefing Loop (Phase 6 of architecture doc)

**Goal:** Daily morning brief that makes Altevra **alive** in Pavle's
daily routine.

**Estimated:** 4-6h.

**Gated on:** Phase 4 + Phase 5 + Phase 6.

### Deliverables

1. **Daily Briefing mode** end-to-end via resident runtime
2. Brain job: `daily_briefing` (configurable cron, default 07:00)
3. Pulls: last updates + active tasks + active goals + recent sessions +
   important decisions + useful research + wiki changes + personal signals
4. Output: `~/Obsidian/Imperium/Daily/YYYY-MM-DD-altevra-brief.md`
   (markdown matching daily_briefing_v1 schema)
5. MCP tool: `get_daily_briefing(date)`
6. **Daily Capture Protocol** — evening capture interactive prompt
   (`altevra capture today`) — 10 questions from architecture doc §15.1
7. Hermes integration — Hermes morning gateway reads
   `altevra get_daily_briefing` and surfaces it to Pavle's morning
   channel

**Exit criteria:** Every morning, Pavle wakes up to a brief that is
**signal not noise** — actionable, project-aware, personal-aware. Evening
capture asks the 10 daily questions.

---

## Phase 8 — Self-Improvement Loop + Skill Factory

**Goal:** Altevra observes itself + generates skills for other agents.

**Estimated:** 8-12h.

**Gated on:** Phase 4 + Phase 7 (loop must be alive before observing it).

### Deliverables — borrowed from Hermes' `background_review.py`

1. **Background review fork** — after every meaningful Altevra event
   (session import, hook turn, research run, wiki edit), spawn a
   resident agent in **review mode** with whitelisted tools
2. Trust ladder enforcement (`VISION.md §4.3`):
   - Auto-applies: research insights, low-risk wiki pages, category creation
   - Review required: sensitive memory, skill proposals, prompt tweaks,
     identity edits, person/relationship pages
3. **Skill factory** for other agents:
   - Observer mode detects repeated tool-call patterns across sessions
   - Proposes new skills with manifest (trigger, command, description)
   - Pavle reviews → Altevra writes skill to target adapter dir
     (`.claude/skills/`, `.codex/skills/`, `.cursor/skills/`, etc.)
4. **Prompt tweak proposals** — observer mode notices when current
   mode prompt produces low-quality output → proposes refined prompt
   with A/B preview
5. New brain jobs: `background_review`, `skill_proposer`,
   `prompt_tweak_proposer`
6. `skill_proposals`, `prompt_tweaks` tables (migrations 023, 024)
7. CLI: `altevra review run`, `altevra skill propose`,
   `altevra prompt tweaks list`

**Exit criteria:** Pavle's repeated workflows trigger skill proposals
within 1 week of installation. Altevra has applied ≥5 of its own
auto-applies and ≥3 Pavle-approved skills exist in other tools.

---

## Phase 9 — Cursor CLI Adapter

**Goal:** Pull in Pavle's Cursor CLI history (50,879 ai_code_hashes).

**Estimated:** 3-4h.

**Gated on:** Phase 0 (existing v0.3.8 import is foundation).

### Deliverables

1. `crates/altevra-cli/src/commands/analyze/parsers/cursor_cli.rs`
2. Read `~/.cursor/ai-tracking/ai-code-tracking.db`:
   - `conversation_summaries` → ImportedSession when populated
   - Group `ai_code_hashes` by `conversationId` → synthesize when no summary
3. Import `~/.cursor/plans/*.plan.md` as memory documents (8 plans on
   Pavle's box)
4. Extend `discover_cursor_jsonls` to route to cursor_cli parser when
   ai-tracking.db exists

**Exit criteria:** `altevra setup analyze-everything` pulls in Pavle's
Cursor CLI history; tracked AI code generations linked back to source
sessions.

---

## Phase 10 — v0.3 Closure + v0.4 Release

**Goal:** Tag v0.3.0, write release notes, public announcement.

**Estimated:** 2-3h.

### Deliverables

1. `CHANGELOG.md` for v0.3 covering Phases 0-9
2. `gh release create v0.3.0` with full notes + ASCII banner
3. README "What's new in v0.3" sekcija
4. Tweet/LinkedIn post draft (Pavle decides whether to publish)
5. Demo GIF (asciinema-recorded `altevra setup analyze-everything` flow)

**Exit criteria:** v0.3.0 GitHub release tagged. README points to it.

---

## Beyond v0.3 — long-arc

(See [VISION.md §5](./VISION.md) for the decades view. Below is the
near-term 6-12 month extension.)

### v0.4 — Knowledge Graph + Cross-Pollination

- Edges between entities (people ↔ projects ↔ decisions ↔ goals)
- Cross-pollination queries: "what's connected to X across all
  categories?"
- Graph visualization (terminal-friendly first; web UI later)

### v0.5 — Pattern Detection

- Sleep ↔ productivity correlation
- Decision → outcome correlation across months
- Annual cycle detection
- Energy / mood ↔ project quality

### v0.6 — Active Briefings

- Smarter than daily brief — proactive surfacing at meaningful moments
  ("you're about to make a decision similar to one from 6 months ago
  that didn't work out")

### v0.7 — Voice & Multi-Modal

- Voice memos imported and transcribed
- Photo capture with auto-tagged context
- Speech-to-thought-to-memory pipeline

### v0.8 — Wearable + Health Integration

- Sleep tracker import
- Heart rate / HRV during deep work sessions
- Mood ↔ biometric correlation

### v1.0 — Stable API Lock

- 12-month API stability guarantee
- Plugin marketplace (skills, connectors, providers)
- Binary releases (cargo-binstall, homebrew tap, .deb / .rpm)
- Public docs site

---

## Cross-cutting concerns (apply across all phases)

### Test discipline

- Every phase must end with ≥10 new tests, total suite green
- Integration tests for new MCP tools (manual JSON-RPC roundtrip)
- Smoke test on Pavle's real data before each commit

### Documentation discipline

- Every new CLI command gets a `--help` entry
- Every new MCP tool gets a 1-line description in `tools/list`
- Every new doc file gets an entry in README "Documentation" section

### Security discipline

- Every external HTTP call documented in side-effects audit
- Personal-category data routes through `local_private` role by default
- Secrets continue to auto-capture before redaction

### Vision gate enforcement

Every phase plan must answer the 6 questions from
[CLAUDE.md §6](./CLAUDE.md):
1. Personal-first parity?
2. Compounds over time?
3. Universal integration?
4. Relevance, not noise?
5. Sovereignty preserved?
6. Identity grows?

If more than one "no" — redesign before building.

---

**Maintenance:** Update this file at the end of every phase. Append
new phases at the bottom. Never delete completed phases — they are the
record of how Altevra got built.

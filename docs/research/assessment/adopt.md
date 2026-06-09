# Hivemind Adoption Map — "Make Altevra Alive" Build List

**Date:** 2026-06-09
**Author:** Claude (Sonnet 4.6), assessment subagent
**Branch:** s0-foundation
**Purpose:** Synthesize Hivemind research docs + current Altevra code state → prioritized, dependency-ordered build list to make Altevra genuinely alive.

---

## 0. TL;DR

Altevra has substantial scaffolding already built — self-improve loop (7 stages), skill-factory proposer mode, improvement signals, proposals table, trust ladder/firewall, resident modes, brain jobs, MCP tools (40+). The S0 foundation fixes (paths, observer reader, working_dir, UTF-8 panic) are partially landed on branch `s0-foundation`. What is MISSING to make it "alive" in the Hivemind sense is:

1. **S0 is not fully done** — `db unify` (A5, the shadow-DB merge) has no implementation yet; observer reader rewrite (A4) IS done per `observer.rs:62-66`; working_dir migration 034 IS landed; absolute paths (`default_db_path()`) ARE fixed.
2. **No Tool Register exposed at session start** — the brain has `get_capabilities` MCP tool but it reads a static JSON file; there is no live tool registry that knows Pavle's actual tools (Imperium Crawl, chatgpt-py, NotebookLM, phone-use, browser-automation, etc.) and no SessionStart hook that injects it unconditionally.
3. **No SessionStart context injection** — `hook_handle session_start` writes a session row and prints `{"session_id": "..."}` but emits ZERO model-visible context. Hivemind's `additionalContext` / RULES/GOALS block is the #1 missing piece for "alive at session open."
4. **Skill factory C4 renderer** — signals and proposals work, but `MarkedForRender` triaged proposals never get rendered (no `altevra skill-factory render` command, renderer reads "undefined" in C4).
5. **SkillOpt bounded-edit optimizer** — Altevra has the signal→proposal→firewall→skill pathway but NOT Hivemind's backward pass: `bounded edit ops + edit budget + slow-update region + anti-sycophancy judge + meta-fingerprint memory`. These are the missing pieces for skills that *improve after being used*.
6. **Daily briefing + proactive notifications** — brain has `DailySummary` job but it writes a stub file; no notification source/rule/delivery contract, no relevance gate, no Obsidian daily-note delivery.
7. **Personal brain layer** — migration 029 has persons/relationships/preferences but no `personal_notes(kind)` table, no NoteCommands CLI, no `interests.yaml` relevance gate.

---

## 1. What exists vs. what to port vs. what is net-new

### EXISTS in Altevra (verified against code)

| Component | File | Status |
|-----------|------|--------|
| 7-stage self-improve loop | `altevra-brain/src/selfimprove.rs` | Working, tested (979 lines, 5 tests pass) |
| Improvement signals (C1 producer) | `altevra-db/src/repositories/improvement_signals.rs` | Working; `signal_for_session` + `signal_for_skill_candidate` |
| Proposals table (028) with firewall | `selfimprove.rs` + `altevra-core/src/selfimprove.rs` | Working; Tier-0/1/2 routing, kill switch, circuit breaker |
| Skill-factory proposer resident mode | `06-skills/resident-agent-modes/skill-factory-proposer.md` | Prompt defined, proposal-only, no auto-apply |
| SI-6 self-write exclusion | `improvement_signals.rs:176-203` | Working; resident-authored sessions excluded |
| Skill propagation watcher | `altevra-skills/src/watcher.rs` | Working; watches `~/.{claude,codex,cursor,hermes,imperium}/skills/` |
| MCP server (40 tools) | `altevra-mcp/src/` | Working; includes `get_agent_bootstrap_packet`, `build_system_prompt`, `get_goals`, `search_memory`, `propose_improvement` |
| Brain jobs (14 kinds) | `altevra-brain/src/jobs.rs` | Scheduler registers all 14 kinds; `DailySummary`, `SelfImproveOrchestrator`, `ObserverScan` all wired |
| Observer reader (post A4) | `commands/observer.rs:62-66` | FIXED — now queries `EventsRepository` + calls `detect_patterns` |
| Absolute paths (post A2) | `altevra-core/src/paths.rs:41-47` | FIXED — `default_db_path()` uses `$HOME`; session pointer keyed by `(tool, hash(cwd))` |
| Working_dir migration 034 | `altevra-db/migrations/034_working_dir.sql` | EXISTS, `SessionRow.working_dir` + `TurnRow.working_dir` wired |
| UTF-8 panic fix (A1) | `recall.rs`, `turn_search.rs` | FIXED per branch changes |
| `get_capabilities` MCP tool | `tools_capabilities.rs` | Reads static JSON at `~/.altevra/state/capabilities.json`; returns hardcoded fallback |
| Review items routing | `selfimprove.rs:547-553` | Working; `persona`/`source_of_truth` → review_item |
| Personal tables | `migration 029` | `persons`, `relationships`, `preferences`, `event_log_personal` |
| Domain policy / exposure gate | `altevra-db/src/repositories/domain_policy.rs` | EXISTS; high-water domains enforced |

### MISSING — needs port from Hivemind or net-new build

| What | From Hivemind | Net-new | Priority |
|------|--------------|---------|----------|
| `db unify` (shadow-DB merge, A5) | No equivalent | Net-new | S0, BLOCKING |
| SessionStart context injection (RULES/GOALS block as `additionalContext`) | `context-renderer.ts` + `session-start.ts` | Adapt | S0→S1.5 |
| Live Tool Register / capability registry with Pavle's actual tools | Partial: Hivemind `hivemind_rules` table | Net-new; extend 023 capability tables | S2 |
| SkillOpt bounded-edit optimizer | `skill-edits.ts` + `skill-proposer.ts` + `success-judge.ts` + `skillopt-meta.ts` | Port to Rust | S2 |
| Anti-sycophancy success judge | `success-judge.ts` | Port to Rust | S2 |
| Meta-fingerprint edit memory | `skillopt-meta.ts` | Port as SQLite table | S2 |
| Event-driven SkillOpt trigger (K-message window on skill invocation) | `skillopt-trigger.ts` | Port | S2 |
| skill-factory render command (`altevra skill-factory render`) | None (Hivemind writes SKILL.md inline) | Net-new (Codex renderer) | S2 |
| Notification source/rule/delivery contract | `src/notifications/` | Port shape, Altevra-specific rules | S4 |
| Proactive surfacing patterns (resume brief, cadence-gated) | `resume-brief.ts`, `referral-invite.ts` | Port patterns | S4 |
| `userVisibleOnly` flag for sensitive personal insights | `types.ts:59` | Port | S4/S5 |
| Atomic dedup (`tryClaim` O_EXCL file) for parallel SessionStart fires | `index.ts:119` | Port | S4 |
| High-precision-or-silent relevance gate | `cold-start-brief.ts:286` | Port discipline | S4/S5 |
| Daily briefing → Obsidian daily note | None in Hivemind | Net-new | S4 |
| `personal_notes(kind)` table + NoteCommands | None in Hivemind | Net-new | S5 |
| `interests.yaml` relevance gate | None in Hivemind | Net-new | S5 |
| Channel matrix (which agent gets which injection type) | `AGENT_CHANNELS.md` | Port discipline | S1.5 |
| `altevra context` CLI for hook-less tools | `src/commands/context.ts` | Net-new | S1.5 |
| Watermark = oldest-mined (not newest) | `skillify-worker.ts:376-384` | Apply to session import cursor | S0.5 |
| `writeJsonIfChanged` idempotency (skip byte-identical hook config rewrites) | `install-codex.ts:246` | Apply to `install_hooks.rs` | S0 |

---

## 2. Gap between current code and "alive" vision

**"Alive" means:** At session open, Altevra unconditionally pushes compact context (active goals, recent decisions, tool register, open proposals) into the agent. During the session, it captures every turn. At session close, it enqueues a signal, runs the observer, clusters signals into proposals, and applies Tier-0 auto-applies. Skills that fail get the backward-pass SkillOpt treatment. Every day, a proactive briefing surfaces what the brain noticed. Personal data (relationships, goals, health) gets equal weight to business data.

**Current gap, in one sentence:** Altevra captures but doesn't push; it has proposals but no renderer; it has a skill watcher but no optimizer; it has a DailySummary job but it writes a stub; it has `get_capabilities` but returns a static hardcoded JSON.

The single highest-leverage gap is **SessionStart context injection** — even with 1157 turns captured, zero context reaches the agent at session open because `hook_handle session_start` only prints `{"session_id":"..."}`. Hivemind's `additionalContext` from `session-start.ts:227-269` is the pattern to copy. This is the difference between Altevra being a passive logger and being an active second brain.

---

## 3. Prioritized, dependency-ordered build list

### P0 — Finish S0 (unblocks everything)

**P0.1 — `db unify` (A5, BLOCKING)**
- No implementation exists. The shadow-DB problem means data is fragmented.
- Effort: ~400-600 lines of Rust.
- What to build: `altevra db unify --dry-run` command with WAL-safe backup + dedup + quarantine.
- Dependency: S0.1 (absolute paths) is done; working_dir (034) is done.
- Head start: None. PLAN.md has full spec including dedup rules for NULL-external_id sessions.

**P0.2 — Regenerate installed hook configs**
- After A2/A5, the hooks in `~/.claude/settings.json` still carry old args (may have `$ALTEVRA_PROJECT`, wrong DB path).
- Action: re-run `altevra install-hooks --tool claude-code` from `$HOME`.
- Effort: CLI invocation, ~5 min.

**P0.3 — `pending_indexing` dead-end**
- 51 rows stuck forever. The embedder job never drains it.
- Either wire `altevra embed run` to drain the queue OR remove the indirection.
- Effort: ~1-2 hours.

**Gate:** `db unify` dry-run + real run on the real DB with `--backup`; `cargo test --workspace` green.

---

### P1 — SessionStart context injection (the "alive at open" fix)

**P1.1 — Render a compact context block in `hook_handle session_start`**

Altevra already has `get_agent_bootstrap_packet` and `build_system_prompt` MCP tools that assemble rich context — but they only fire when the agent *chooses* to call them. This must flip to unconditional push.

Port Hivemind's `context-renderer.ts` pattern:
- `hook_handle session_start` queries (async, best-effort, <3s timeout):
  1. Recent open goals (top 5) from `goals` / `tasks` / MCP `get_goals`
  2. Last 3 applied decisions from proposals/memory
  3. Tool register summary (what tools Altevra knows about)
  4. Any open review items (proposals awaiting Pavle)
- Format as a compact block, inject into the hook output's `additionalContext` field (Claude Code reads this as `<system-reminder>`).
- NEVER throw/exit non-zero if this fails. Degrade to `{"session_id": "..."}` only.
- Gate injection by tool: Claude Code gets `additionalContext`; Codex gets minimal/nothing (user-visible terminal, Hivemind lesson from `codex/session-start.ts:116-126`).

Port Hivemind's **two-layer prompt-injection defense**:
- Write-time: reject newlines in rule/goal text (max 2000 chars per item).
- Render-time: sanitize line terminators before inject.

Effort: ~200-300 lines of Rust in `hook_handle.rs` + a shared `context_renderer.rs` module.
Head start: `get_agent_bootstrap_packet` is the correct API surface; reuse its data assembly logic.

**P1.2 — `altevra context` CLI (for hook-less tools)**

Hivemind's `hivemind context` CLI that pi/openclaw call manually to get the RULES/GOALS block. Altevra equivalent: `altevra context --tool hermes` that prints the same compact block to stdout. Hermes can call it on session start via its own hooks.

Effort: ~50 lines.

**P1.3 — Channel matrix (docs + code)**

Document per-tool injection behavior. Avoid blindly injecting big blocks into Codex (clobbers TUI). This is the `AGENT_CHANNELS.md` discipline ported as a Rust config struct.

---

### P2 — Live Tool Register / Capability Registry (exposed at session start)

The existing `023_capability.sql` schema has `capability_records`, `adapter_dossiers`, `capability_grants`. `get_capabilities` returns a static fallback JSON. Altevra needs a *live* tool register that knows Pavle's actual tools.

**P2.1 — Populate the registry with Pavle's known tools**

Tools to register: Claude Code, Codex, Cursor CLI, Hermes, Antigravity, Imperium Crawl, chatgpt-py (via ChatGPT skill), NotebookLM, phone-use, browser-automation, computer-use, transcribe, content-pipeline, imperium-cloud, graphify, and all installed skills from `~/.claude/skills/`.

Each entry: `tool_id`, `display_name`, `kind` (adapter/skill/external), `can`/`cannot`/`unverified` capabilities, `install_path`, `last_seen_at`.

**P2.2 — Auto-update from skill watcher**

The `altevra-skills/src/watcher.rs` already watches `~/.{claude,codex,cursor,hermes,imperium}/skills/`. On each cycle, update the registry with newly seen skills.

**P2.3 — Surface in SessionStart context block (P1.1)**

The P1.1 block should include a 1-line summary of the tool register: "Tools known: Claude Code, Codex, Hermes, Imperium Crawl, 47 skills."

Effort: ~300 lines total across P2.1-P2.3.
Head start: 023 schema + `tools_capabilities.rs` + watcher already exist.

---

### P3 — Skill Factory renderer + SkillOpt backward pass (S2)

**P3.1 — `altevra skill-factory render --proposal <id>` (C4)**

The self-improve loop already marks skill proposals `triaged` (`MarkedForRender`). C4 picks them up. Currently C4 does not exist.

Build:
- Read a triaged `kind='skill'` proposal's `evidence_refs` (raw session/turn refs).
- Build a bounded raw-replay packet (conserve tokens, PLAN.md S2.3 spec).
- Route to `strong_reasoner` (Codex via OAuth).
- Validate frontmatter + sections; secret/PII scan.
- Stage to `docs/generated/skills/<slug>/SKILL.md` (never write to install dir directly).
- Default `--dry-run`.

**P3.2 — SkillOpt bounded-edit optimizer (port from Hivemind)**

This is the crown jewel from Hivemind's `skill-edits.ts`. Port to Rust, ~400 lines:

```rust
// Pure, deterministic, unit-testable — no I/O
enum EditOp { Append, InsertAfter, Replace, Delete }
struct Edit { op: EditOp, target: String, content: String }

fn apply_edits(body: &str, edits: &[Edit]) -> String { /* exact substring anchors */ }
fn select_edits(edits: &[Edit], budget: usize) -> &[Edit] { &edits[..budget.min(edits.len())] }

const SU_START: &str = "<!-- SLOW_UPDATE_START -->";
const SU_END: &str = "<!-- SLOW_UPDATE_END -->";
fn targets_protected(body: &str, edit: &Edit) -> bool { /* overlap check */ }
```

**P3.3 — Anti-sycophancy success judge**

Port `success-judge.ts` to Rust: `judge_success(window: &str) -> SuccessVerdict`. Conservative-on-failure: unparseable/errored → `success=1`. Route to `cheap_worker`. The single question: "Was the task done CORRECTLY, ignore whether the user seemed happy?"

**P3.4 — Meta-fingerprint edit memory**

Port `skillopt-meta.ts` as a SQLite table (cleaner than JSONL):
```sql
CREATE TABLE skill_edits (
    id TEXT PRIMARY KEY,
    skill_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL, -- order-independent hash of edit set
    ops_json TEXT NOT NULL,
    status TEXT DEFAULT 'proposed', -- proposed | applied | reverted
    proposed_at TEXT NOT NULL,
    UNIQUE(skill_id, fingerprint)
);
```

**P3.5 — Event-driven SkillOpt trigger**

When `post_tool_use` captures a Skill invocation (`tool_name` is a skill name), open a K-message window. On the next user prompt, check if the skill was invoked and the reaction is negative. If yes, spawn the `SkillOpt` backward pass (judge → propose edit → route to review queue per trust ladder). Per-session pending state keyed by `(session_id, skill_name)`.

Effort total for P3: ~600-800 lines. Head start: proposals table, `selfimprove.rs` firewall, `altevra-llm` role routing.

---

### P4 — Daily briefing + proactive notifications (S4)

**P4.1 — Notification source/rule/delivery contract**

Port Hivemind's three-layer split to Rust:

```rust
// Pure, IO-free rule
trait NotificationRule {
    fn evaluate(&self, ctx: &NotificationContext) -> Option<Notification>;
}

struct Notification {
    id: String,
    severity: Severity,
    title: String, // <= 80 chars
    body: String,  // 1-3 lines
    dedup_key: serde_json::Value,
    transient: bool,
    user_visible_only: bool, // CRITICAL for personal/sensitive insights
}
```

**P4.2 — Concrete notification rules (Altevra-specific)**

Port Hivemind's patterns, adapted to personal brain:
- **Decision-staleness rule:** "You made decision X N months ago — still applies?" Key on `{decision_id, version}`.
- **Relationship-cadence rule:** "Haven't mentioned [person] in N weeks." Cadence-gated (≥ 6 weeks), `userVisibleOnly=true` (personal).
- **Resume-brief rule:** "Where you left off" — latest session with open Next Steps.
- **Open proposals rule:** "N skill proposals await review."
- **Pattern-detected rule:** "Altevra noticed: [insight_title]." `userVisibleOnly=true` if personal.

Discipline: **high-precision-or-silent**. Every rule returns `null` rather than surface a weak signal. This is the relevance gate.

**P4.3 — Delivery adapters**

- Claude Code: `additionalContext` (model-visible) + `systemMessage` (user-visible) — dual channel from `emitClaudeCode` pattern.
- Obsidian daily note: append a `## Altevra Brief` section — the human-canonical channel.
- `userVisibleOnly` notifications go ONLY to `systemMessage` and Obsidian, never to `additionalContext`.

**P4.4 — Atomic dedup (O_EXCL claim file)**

Port Hivemind's `tryClaim`: `O_CREAT|O_EXCL` on a claim file at `~/.altevra/state/notifications/claims/{id}.lock`. Two parallel SessionStart hooks can't double-fire. Cheap and correct.

**P4.5 — `DailySummary` job wires to the notification pipeline**

`JobKind::DailySummary` fires hourly but acts only at 23:00. Currently it writes a stub. Wire it to: evaluate all notification rules → dedup → emit to Obsidian daily note at `~/Obsidian/Imperium/Daily/YYYY-MM-DD.md` as `## Altevra Brief`.

Effort: ~500-700 lines. Head start: `JobKind::DailySummary` exists, `altevra-brain/src/resident.rs`, MCP `get_goals` tool, existing Obsidian write path.

---

### P5 — Personal brain layer (S5)

**P5.1 — `personal_notes(kind)` table**

PLAN.md says: extend 029 (persons/relationships/preferences stay canonical) + add one `personal_notes(kind)` table for Decision, Learning, Idea, Goal, Mood, Health, Place, Reference, Habit, Routine, Value, IdentityShift, LifeEvent. FK links to 029 where a note references a person/relationship.

**P5.2 — NoteCommands CLI**

`altevra note add <kind> "<text>"` and `altevra note list [--kind <kind>]`. Maps to 029 canonical tables (person/relationship/preference) and the new `personal_notes` table.

**P5.3 — `interests.yaml` relevance gate**

`~/.altevra/interests.yaml` lists Pavle's active interests + active goals. Research jobs (`ResearchFetcher`, `ProjectResearchSweep`) check this gate before pulling external content. Off-interest items dropped silently. On-interest items get a `relevance: high` tag.

**P5.4 — `userVisibleOnly` enforcement in personal notifications**

Relationship staleness, health patterns, mood correlations — all personal insights must carry `user_visible_only=true` so they never reach model context (sovereignty axiom). Enforced in the delivery adapter (P4.3).

Effort: ~400 lines. Head start: migration 029 tables, `domain_policy.rs`, `event_log_personal`.

---

## 4. Dependency order summary

```
S0 remaining:
  db_unify (P0.1) ← blocks everything stable
  hook config regen (P0.2)
  pending_indexing fix (P0.3)

S0.5:
  Codex/claude-code import parser fixes
  Import CLI wiring
  Watermark = oldest-mined in import cursor

S1: (model runtime — Ollama + per-role routing — blocks P3/P4)

P1 SessionStart injection ← independent of S1, can land after S0
  P1.1 context block in hook_handle
  P1.2 altevra context CLI
  P1.3 channel matrix

P2 Tool Register ← after P1, uses watcher
  P2.1 populate registry
  P2.2 watcher auto-update
  P2.3 surface in context block

S2 (skill factory):
  P3.1 renderer (C4) ← requires S1 Codex route
  P3.2 SkillOpt bounded edit ← pure Rust, no model dependency
  P3.3 success judge ← needs cheap_worker (S1)
  P3.4 meta-fingerprint table ← pure SQL, no model dependency
  P3.5 SkillOpt trigger ← requires P3.2 + P3.3

S4 (observer + briefing):
  P4.1 notification contract ← after S0
  P4.2 notification rules ← after P4.1
  P4.3 delivery adapters ← after P4.1
  P4.4 atomic dedup ← with P4.3
  P4.5 DailySummary wired ← after P4.1-P4.4

S5 (personal brain):
  P5.1 personal_notes table ← extends 029
  P5.2 NoteCommands CLI ← after P5.1
  P5.3 interests.yaml gate ← after P5.1
  P5.4 userVisibleOnly enforcement ← with P4.3
```

---

## 5. What Altevra already does better than Hivemind (keep as-is)

- **Single-binary capture** — no per-tool JS bundle to keep in sync.
- **Backup-before-write** in `install_hooks.rs:245-262` — Hivemind has none.
- **SI-6 self-skip** — Hivemind has no concept of this (it's a team tool, not its own subject).
- **Smart project scoping** — Hivemind is user-global only; Altevra scores repos and writes project-local configs.
- **Trust ladder** — Hivemind auto-publishes SkillOpt edits with no human gate. Altevra inverts this: all SkillOpt proposals route to review items by default; only Tier-0 non-sensitive changes auto-apply.
- **40+ MCP tools** vs Hivemind's 3 — richer recall surface.
- **Local-first sovereignty** — Hivemind is cloud-SaaS; Altevra is `local_private` by axiom.
- **Personal + business parity** — Hivemind is coding-tool-only; Altevra covers life domains.

---

## 6. Effort sizing summary

| Item | Effort | Dependency |
|------|--------|------------|
| P0.1 db unify | M (400-600 lines) | None |
| P0.2 hook config regen | XS (CLI invocation) | P0.1 |
| P0.3 pending_indexing | S (1-2h) | None |
| P1.1 SessionStart injection | M (200-300 lines) | S0 done |
| P1.2 altevra context CLI | XS (50 lines) | P1.1 |
| P2 Tool Register | M (300 lines) | P1.1 |
| P3.2 SkillOpt bounded edit (pure Rust) | M (400 lines) | None — pure functions |
| P3.3 success judge | S (100 lines + S1 cheap_worker) | S1 |
| P3.4 meta-fingerprint table | S (50 lines SQL + repo) | None |
| P3.1 skill-factory renderer | M (300 lines + S1 Codex route) | S1 + P3.4 |
| P3.5 SkillOpt trigger | S (150 lines) | P3.2 + P3.3 |
| P4.1-P4.4 notification framework | L (500-700 lines) | S0 |
| P4.5 DailySummary wired | S (100 lines) | P4.1-P4.4 |
| P5.1-P5.4 personal brain layer | M (400 lines) | S0 |

**Total to full "alive" state: ~3000-4000 lines of Rust, staged across S0 finish → S5.**

---

## 7. The single highest-value first slice

If forced to pick ONE thing that makes Altevra feel alive to Pavle immediately (after S0 is done):

**P1.1 — SessionStart context injection.**

30 minutes to implement, 3 years of payoff. Every time Claude Code opens, Altevra pushes: active goals, last 3 decisions, open proposals, tool count. The model has context before the first word is typed. This is the difference between a passive logger and an external mind.

The second-highest: **P4.2 relationship-cadence notification rule**. "Haven't mentioned Elena/Srđan/Đorđe in N weeks" — this is the personal-brain feature Hivemind can never have and Altevra can ship in ~100 lines after P4.1.

---

*Read alongside: `/home/pavle/projekti/ai-tooling/altevra/docs/research/hivemind/00-SYNTHESIS.md` (Hivemind adoption decision), `PLAN.md` (S0-S6 full spec), `ALTEVRA-DEEP-AUDIT-2026-06-07.md` (bug board).*

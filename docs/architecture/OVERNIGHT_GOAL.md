# Altevra Overnight Autonomous Goal

Status: **ACTIVE — autonomous execution**
Set: 2026-06-01 ~03:2x by Pavle (explicit overnight directive)
Authority docs (read first, in order): `RECONCILIATION.md` (R1–R14) → `BUILD_TASKS.md` (T0.x–T9.x) → `contracts/P0_CONTRACTS.md` → working draft §1–§7.

---

## Mission

Implement the Altevra core **end-to-end, autonomously, through dynamic workflows**, so that **tomorrow morning Pavle only has to drop in API keys** and the resident agents come alive. Everything that does NOT need keys must be built, committed, baseline-green, and **live-tested in a second tool**.

**Scope decision (Pavle): PUSH THROUGH EVERYTHING — don't stop.** Go through all 73+ tasks in dependency order. Anything that needs an LLM/network is implemented against a **noop/stub provider** so the contract + tests run without keys; the real provider activates when Pavle adds keys. Do not stop at "core done" — keep going until the workflow exhausts what can be built keylessly.

**Why this fits "just add keys":** P0.0→P0.4 are no-LLM, no-network, deterministic by design (tag/FTS5/graph retrieval — R12, no embeddings). P0.5+ (resident runtime, self-improve, skill factory, domain LLM-classification) are built with the noop provider; keys flip them from stub→live.

---

## Done definition (per phase — commit each task atomically)

- **P0.0** — enums (6-level sensitivity R1, Domain R3, status families R2), template system + builtin templates (R13), fixtures, contract-validation test. Compiles, tests green.
- **P0.1 vertical loop** — `altevra p0-vertical-smoke --fixture fixtures/p0 --json` passes: capture → PreWriteSafetyGate (+TemplateGate +mandatory tags, R13) → persist (envelope migration 019) → Obsidian mirror → **tag/FTS5/graph** packet (R12, NO vectors) → exposure_decision audit → review_item. **Leak suites = 0** (secret/scope/sensitivity). Deterministic JSON.
- **P0.2 + P0.3** — capability registry (honesty: `supported` needs evidence), drift→review, control-plane CLI/MCP verbs, `doctor` extended (+ tag-coverage/template-conformance).
- **P0.4** — tag-first + FTS5 + graph retrieval full (R12), golden eval green (non-embedding cases), multilingual via FTS5 analyzer + tags.
- **P0.5** — model role routing + **noop provider** (R10); resident_mode registry (extend brain scheduler/JobKind); resident_run; dry-run resident emits schema-valid, review-routed proposals. NO real model calls.
- **P0.6** — unified proposal/improvement_signal/prompt objects; 7-stage self-improve loop; **runaway firewall** (budgets/circuit-breaker/cap/cooldown/eval-gate/constitutional-lock/kill) in Rust below the LLM.
- **P0.7** — skill factory (propose→render→install→monitor) + Hermes ToolAdapter (R10 Q7).
- **P0.8** — domain_policy (9 builtins), retention/lifecycle engine, project lifecycle (archive-demotion/scope-promotion/compaction), export/forget/legal-hold (human-presence R4). LLM-classification stubbed.
- **P0.9** — per-domain cloud_sync ceiling enforcement, tombstone model. No daemon.

## Real integration test (Pavle: spawn live in herdr)

After P0.3 (and again after each later phase that touches the connection):
1. Build release binary; symlink `~/.local/bin/altevra`.
2. **Spawn a fresh Claude Code agent in herdr** (new workspace), wire Altevra MCP (`altevra serve`) + hooks, call `get_agent_bootstrap_packet` / `get_context_packet` / `search_memory`, confirm they return correct, ceiling-gated results on real fixture data.
3. **Spawn / open Cursor** similarly; verify adapter-rendered config + MCP work.
4. Record each live test result in `docs/architecture/REAL_TEST_LOG.md` (pass/fail + what was checked). A failed live test is a blocker to that phase's sign-off.

---

## Hard rules (non-negotiable during autonomous run)

1. **Commit per task**, atomic, descriptive message. Never leave the tree red.
2. **Baseline must stay green** after each phase: `cargo fmt --check && cargo test && cargo build && cargo clippy --workspace -- -D warnings`.
3. **NEVER real secrets** — only synthetic/fake in fixtures (verify they never persist raw).
4. **No external side-effects without Pavle** — no deploy, no `git push`, no email/DM, no customer contact, no payments. Local commits only.
5. **Codex breaker re-run** on the safety crate (`ingest_guard`/`exposure_gate`) before merging P0.1 safety code (R11). If Codex is out of credits, record it and proceed, flagging the gap.
6. **Follow R1–R14 exactly.** No vectors in retrieval (R12). Every faced write goes through TemplateGate + mandatory tags (R13). Small focused resident modes, never a monolith (R14). New integrations are modules, not core edits (MOD-1).
7. **No `secret`-classified object bodies**; only `secret_sighting` fingerprints (R1).

## Blocker protocol (don't stall the night)

If a task hits a decision genuinely needing Pavle (not resolvable from RECONCILIATION/code/sensible default):
- Write it to `docs/architecture/BLOCKERS.md` (task id, the question, options, your recommended default).
- **Apply your recommended default and keep going** if it's reversible and low-risk; otherwise skip to the next independent task.
- Never halt the whole run on one blocker.

## Progress + resume

- Maintain `docs/architecture/PROGRESS.md`: task id → status (done/in-progress/blocked) + commit hash. Update as you go so any resume picks up exactly where it stopped.
- Morning handoff: a short summary at the top of PROGRESS.md — what's done, what's live-tested, what awaits keys, any blockers.

---

## Morning state Pavle should find

A repo where: the deterministic core works and is committed; `p0-vertical-smoke` passes; retrieval works tag-first with zero leaks; the connection to Claude Code (and Cursor) is live-tested and logged; resident/self-improve/skill-factory/domains are built against the noop provider; and a single `PROGRESS.md` tells him exactly what flips live the moment he adds keys.

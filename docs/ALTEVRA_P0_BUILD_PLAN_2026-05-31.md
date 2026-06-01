# Altevra P0 Build Plan — Self-Improving Foundation

Date: 2026-05-31
Repo: `/home/pavle/projekti/ai-tooling/altevra`
Status: baseline verified

## Baseline verification

Commands run from repo root:

```bash
cargo fmt --check
cargo test
cargo build
cargo clippy --workspace -- -D warnings
```

Results:

- `cargo fmt --check` — PASS
- `cargo test` — PASS
  - 500+ test cases observed across workspace crates
  - 1 ignored keyring test expected: requires OS keyring
- `cargo build` — PASS
- `cargo clippy --workspace -- -D warnings` — PASS
- `git status --short` before plan write — clean

Important note: `ROADMAP.md` still lists dead-code/clippy warnings as open, but current clippy with `-D warnings` passes. Treat that roadmap item as stale and update it when touching roadmap.

## Product direction clarified

Hermes-like self-improvement is a core Altevra pattern:

```text
capture usage/events
→ detect repeated workflow / drift / pain
→ resident model proposes improvement
→ review gate for risky changes
→ render/update skill/hook/context/prompt
→ monitor usage
→ patch/deprecate when stale
```

This is not optional polish. It is the compounding loop that makes Altevra a living thinking OS instead of a static context DB.

## Build rule

Pick exactly one P0 build unit at a time.

Do not build dashboard, broad connectors, or external API-heavy resident runtime until foundations are safe.

Every unit must end with:

```bash
cargo fmt --check
cargo test
cargo build
cargo clippy --workspace -- -D warnings
```

## P0 build units

### P0.1 — Data model upgrade for thinking OS

Owner recommendation: Claude Code primary, Hermes reviewer.

Goal: make durable objects rich enough for resident reasoning and self-improvement.

Scope:

- Extend existing `tasks/goals/decisions` foundation where needed.
- Add/verify repository coverage for:
  - insight cards
  - review items
  - context packet sources
  - skill proposals
  - resident runs / resident outputs
  - secret sightings metadata if not already represented enough
- Ensure durable rows have provenance, sensitivity, status/staleness, timestamps.
- Add roundtrip tests similar to `crates/altevra-db/tests/repository_roundtrip.rs`.

Acceptance:

- migrations from empty DB pass
- repository roundtrip tests pass
- no raw secrets in durable text fixtures
- `cargo fmt/test/build/clippy` green

Why first: resident/self-improve needs structured places to write proposals before any autonomous behavior exists.

### P0.2 — Secret safety gate before deeper capture

Owner recommendation: Codex implementation, Claude Code review, Hermes policy check.

Goal: guarantee prompt/tool/event ingestion cannot leak raw secrets into DB/context/embeddings.

Scope:

- Audit capture paths: turn record, hook handle, session import, file history, memory ingest, context packet, embeddings queue.
- Add fake-secret regression tests covering each ingestion boundary.
- Ensure secret detection/capture/redaction happens before storage/index/context.
- Ensure raw secret can only live in keyring/encrypted backend and not in normal DB text.

Acceptance:

- fake OpenAI/GitHub/AWS/JWT/DB URL secrets absent from DB/context packet/embedding text
- secret handle/fingerprint metadata exists where relevant
- audit/review event exists for secret sighting
- baseline suite green

Why second: more capture without this is dangerous.

### P0.3 — CLI control plane completeness pass

Owner recommendation: Claude Code primary.

Goal: terminal agents can operate Altevra without relying on MCP.

Scope:

- Commands should have `--json` where agent-facing.
- Verify/fill gaps for:
  - context packet compile/show
  - tasks/goals/decisions/learnings
  - memory search/propose
  - skills list/propose/render/check/refresh
  - secrets metadata/grant/run/audit
  - resident run once/dry-run
  - review list/show/approve/reject
  - doctor

Acceptance:

- one shell smoke script can exercise core commands on temp vault
- JSON outputs parse via `jq` or Rust tests
- MCP remains adapter, not primary implementation

### P0.4 — Context packet compiler v1

Owner recommendation: Claude Code implementation, Hermes acceptance review.

Goal: produce safe bounded context packets per agent/tool/project/intent.

Scope:

- Inputs: `agent_kind`, `tool`, `project`, `intent`, `token_budget`, `sensitivity_ceiling`.
- Sources: updates, tasks, goals, decisions, learnings, memory/wiki, research, skills, repo AGENTS.md.
- Log packet source refs.
- Strict redaction before output.

Acceptance:

- no raw secret can appear
- token budget respected approximately
- packet includes source refs, not mystery summaries
- CLI + MCP share same core compiler

### P0.5 — Resident reasoning dry-run foundation

Owner recommendation: Claude Code primary, Codex test pass.

Goal: resident modes can run in dry-run/proposal mode with schema validation before autonomous writes.

Scope:

- Resident mode registry.
- Model client abstraction already exists in `altevra-llm`; use role routing only if already safe, otherwise stub/noop provider for tests.
- Resident run logging.
- Output schema validation.
- First safe modes:
  - insight_synthesizer
  - skill_factory_proposer

Acceptance:

- fixture events produce insight_cards and skill_proposals
- protected outputs route to review queue
- no direct writes to protected memory without approval flag

### P0.6 — Hook ingestion hardening

Owner recommendation: Codex primary, Claude Code review.

Goal: capture real agent usage broadly and safely.

Scope:

- Canonical hook event schema.
- Verify adapters send consistent event names/payloads.
- Test prompt/tool/file/session fixtures from Claude Code, Codex, Cursor, Antigravity, Hermes where available.
- Collapse noisy parser warnings into summary output.

Acceptance:

- prompt + tool-call + file edit fixture creates safe event rows
- parser noise does not break clean JSON mode
- existing adapter tests stay green

### P0.7 — Skill factory proposal/render loop

Owner recommendation: Hermes spec + Claude Code implementation.

Goal: Altevra detects repeated workflows and proposes reusable skills.

Scope:

- Repeated workflow detector v0.
- Skill proposal schema.
- Review queue integration.
- Renderer abstraction for Hermes and generic `.agents/skills` targets.
- Usage tracking so skills can be patched/deprecated.

Acceptance:

- fixture repeated workflow creates one deduped skill proposal
- approval renders skill to target dir
- generated skill has trigger, steps, commands, pitfalls, verification

## Agent split

### Hermes

Use Hermes for:

- product boundary enforcement
- Obsidian/daily/decision updates
- self-improvement loop design
- safety/policy review
- final acceptance summaries
- GTM discipline reminders when build starts drifting

Do not use Hermes as the primary deep repo refactor agent unless task is small and local.

### Claude Code

Use Claude Code for:

- multi-file Rust implementation
- migrations + repository design
- CLI/MCP shared core refactors
- resident/context architecture
- final integration pass

Best for P0.1, P0.3, P0.4, P0.5, P0.7.

### Codex

Use Codex for:

- narrow test-heavy implementation
- safety regression suites
- parser/hook hardening
- clippy/fmt cleanup
- independent review of Claude patches

Best for P0.2, P0.6, and test/review passes on every unit.

## Immediate next recommended task

Do P0.1 first, but scope it tightly:

```text
Implement data model + repository roundtrip support for skill_proposals, review_items, context_packet_sources, and resident_runs only.
Do not implement resident execution yet.
Do not implement external model calls yet.
```

This creates the landing zone for self-improvement without making the system autonomous too early.

## Safety gates

- No commit/push/deploy without Pavle approval.
- No raw secrets in DB/context/embeddings.
- No protected memory/identity/relationship/decision changes without review item.
- No new external model/API dependency in P0.1.
- Do not broaden into dashboard or connectors.

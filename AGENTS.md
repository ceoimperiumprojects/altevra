# Altevra Agent Rules

## Read first

Before coding or architecture work in this repo, read these in order:

1. `docs/architecture/ALTEVRA_ARCHITECTURE_CONSTITUTION.md`
2. `docs/architecture/ALTEVRA_RUNTIME_MODEL_AND_SKILL_FACTORY_DOCTRINE.md`
3. `docs/architecture/ALTEVRA_ARCHITECTURE_WORKING_DRAFT.md`
4. `ALTEVRA_PRODUCTION_ARCHITECTURE_V5.md` for product/feature vision only; its Postgres/pgvector storage sections are superseded by SQLite local-first P0 docs.
5. `ALTEVRA_NEXT_ARCHITECTURE_RESIDENT_AGENT_WIKI_PERSONAL_BRAIN.md` for resident-agent / wiki / personal-brain vision.

If these disagree, current precedence is:

```txt
Pavle explicit correction in current session
> ALTEVRA_RUNTIME_MODEL_AND_SKILL_FACTORY_DOCTRINE.md
> ALTEVRA_ARCHITECTURE_CONSTITUTION.md
> ALTEVRA_ARCHITECTURE_WORKING_DRAFT.md
> ALTEVRA_NEXT_ARCHITECTURE_RESIDENT_AGENT_WIKI_PERSONAL_BRAIN.md
> ALTEVRA_PRODUCTION_ARCHITECTURE_V5.md
```

## Current product doctrine

Altevra is Pavle's local-first external mind / Agent OS.

It records real work, preserves raw trace, retrieves relevant context, runs internal resident modes, creates proposals, and syncs useful skills/prompts/tools into attached AI tools.

Altevra is not only an external recorder for Hermes/Claude. It has its own internal runtime:

- `cheap_worker` LLM — cleanup, classification, clustering, candidate detection.
- `strong_reasoner` LLM — synthesis, conflict detection, proposal quality, architecture/wiki reasoning.
- `local_private` route — high-water/private domains stay local/private.
- `embedding_model` — semantic retrieval and clustering support, not decision-making.
- Codex/GPT renderer — attached high-quality renderer/reviewer that replays raw trace and writes final skill drafts.

## Skill factory law

The skill factory must be Altevra-native:

```txt
raw session/tool/file trace
→ retrieval/indexing
→ cheap/local skill_candidate signals
→ strong_reasoner triaged skill proposal
→ Codex/GPT raw-trace replay + SKILL.md draft
→ validation/safety/dedupe
→ staged install/sync into target adapters
```

Non-negotiables:

1. Raw trace is source of truth.
2. Candidate/proposal rows are pointers + metadata, never lossy replacements.
3. Cheap models may suggest; they do not author final skills.
4. Embeddings retrieve; they do not decide.
5. Codex/GPT must inspect raw refs before rendering final `SKILL.md`.
6. Install/sync is separate from proposal/render and remains policy/review gated.

## Implementation rules

- Rust-first, CLI-first, local-first, MCP-compatible.
- SQLite local-first is P0 canonical; Postgres/pgvector is future optional adapter only.
- Keep changes scoped, testable, and honest about unimplemented pieces.
- Every architecture claim should map to tests, fixtures, or smoke scripts.
- Never silently overwrite managed/drifted files.
- Never commit/push/deploy without Pavle approval.
- Never read/print secrets, `.env` values, tokens, credentials, or browser/session data.
- If a task touches high-water/private domains, route local/private and preserve redaction/sensitivity gates.

## Session startup

Run:

```bash
altevra agent bootstrap --tool <tool> --project altevra --json
altevra updates --project altevra --since last-session --json
```

Then inspect current git status before edits:

```bash
git status --short
```

Do not revert unrelated user/agent changes. If the working tree is dirty, isolate your changes and report exactly what you touched.

## Tests

Use focused tests while iterating, then broader verification before calling work green:

```bash
cargo fmt --all
cargo test --workspace
```

If changing lint-sensitive Rust, also run the relevant clippy command if the task requires production hardening.

## Current state snapshot

This file was refreshed on 2026-06-06 because prior AGENTS.md guidance was stale and implied "no external model/API integration" from an early P0 phase. The current architecture includes internal model routing and attached Codex/GPT rendering as a first-class part of Altevra's self-improvement / skill-factory loop.

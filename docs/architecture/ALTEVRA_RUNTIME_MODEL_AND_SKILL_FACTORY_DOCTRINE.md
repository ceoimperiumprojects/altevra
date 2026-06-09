# Altevra Runtime Model + Skill Factory Doctrine

Status: architecture doctrine / Pavle correction captured
Date: 2026-06-06
Owner: Pavle + Altevra agents

## 0. Core correction

Altevra is not just an external recorder that later asks some outside assistant to make skills.

Altevra has its own internal intelligence runtime attached to the same raw capture layer that Hermes/Claude/Codex use from outside.

The intended runtime topology is:

```txt
External agents/tools
Hermes · Claude Code · Cursor · Antigravity · OpenCode
        │ hooks / MCP / CLI / session imports
        ▼
Raw trace layer
sessions · turns · tool calls · tool outputs · model outputs · file changes · events
        │
        ├── lexical/structured retrieval (SQLite/FTS/BM25/graph)
        ├── embedding model for semantic retrieval
        ▼
Altevra resident intelligence runtime
cheap/local LLM      strong/reasoning LLM      local_private route
        │                 │                    │
        └──── triage / synthesis / curation / proposals ────┐
                                                              ▼
Codex/GPT renderer attached to Altevra runtime
raw-trace replay → draft SKILL.md → validate → place/sync into target adapter dirs
```

This means the skill factory is an Altevra-native loop, not a Hermes-only or human-only workflow.

## 1. Runtime roles

### 1.1 cheap_worker LLM

Purpose: cheap/simple processing.

Allowed work:
- classify turns/sessions
- detect repeated workflow candidates
- clean metadata
- extract structured hints
- cluster similar tool-call patterns
- create `skill_candidate` / `improvement_signal` rows that point to raw trace

Not allowed:
- final `SKILL.md` authorship
- overwriting skills
- treating summaries as source of truth
- touching locked safety/identity/policy prompts

### 1.2 strong_reasoner LLM

Purpose: higher-quality reasoning over bounded context packets.

Allowed work:
- synthesize wiki pages
- detect conflicts
- prioritize proposals
- write review-ready proposal bodies
- decide when a candidate should be escalated to Codex/GPT renderer
- produce architecture/product reasoning when context packet is sufficient

Not allowed:
- bypass review gates
- expose high-water/private context to non-local routes
- replace raw trace with generated summaries

### 1.3 embedding_model

Purpose: semantic retrieval only.

Allowed work:
- embed memory chunks / sessions / wiki pages / skills / proposals
- support recall and hydration
- help candidate clustering with nearest-neighbor search

Not allowed:
- decision-making
- final summarization
- skill writing
- exposure of restricted/private data to cloud embedding providers unless policy explicitly allows it

### 1.4 local_private

Purpose: private/local reasoning route for high-water domains.

Used when content touches:
- personal
- relationship
- health
- legal
- financial
- client/private business data
- secrets-adjacent traces

Rule: if content scan or domain says high-water, route local/private regardless of metadata label.

### 1.5 Codex/GPT renderer

Purpose: final skill authoring and code-level reasoning.

Codex/GPT is attached to Altevra similarly to Hermes/Claude, but it is used as the high-quality renderer/reviewer for skills and complex implementation reasoning.

Allowed work:
- open raw trace refs (`session:*`, `turn:*`, file changes, tool outputs)
- inspect existing skills to dedupe/refine
- generate `SKILL.md` drafts
- validate frontmatter/format/contracts
- place staged skill output into the correct target location after policy says it is allowed
- create patch proposals for existing skills

Not allowed:
- fabricate from cheap summaries only
- skip raw evidence replay
- silently install/share/push external changes without approval/policy gate
- bypass safety gates

## 2. Skill factory pipeline

The intended skill factory flow is:

```txt
1. Capture everything raw
   prompts, model outputs, tool calls, tool outputs, file changes, commands, errors, session metadata

2. Index and preserve
   raw trace stays canonical; redacted/exposable copies may be derived, never destructive

3. Cheap/local triage
   cheap_worker scans recent traces and emits pointer-only candidates:
   - kind: skill_candidate
   - source_ref: session:<id>
   - evidence_refs: [session:<id>, turn:<id>, file_change:<id>, ...]
   - summary: short reason why this may become a skill

4. Strong reasoning synthesis
   strong_reasoner clusters candidates, dedupes against existing skills, and produces triaged `skill` proposals.

5. Codex/GPT render
   Codex/GPT receives proposal + raw refs, replays evidence, and writes the actual skill draft.

6. Validate
   run schema/frontmatter validation, safety scan, dedupe check, and target adapter compatibility.

7. Stage/install/sync
   depending on policy and approval, write into:
   - Altevra canonical skill store
   - generated target adapter dirs (Claude Code, Codex, Hermes, Cursor, Antigravity, etc.)

8. Monitor
   future sessions observe whether the skill helped; bad/stale skills create improvement proposals.
```

## 3. Non-negotiable invariants

1. Raw trace is source of truth.
2. A candidate/proposal is only metadata + pointers, never a replacement for raw evidence.
3. No final skill may be rendered unless Codex/GPT or equivalent strong renderer has direct access to raw trace refs.
4. Cheap models may suggest, cluster, and sort; they do not author final durable skills.
5. Embeddings retrieve; they do not decide.
6. High-water/private data routes local/private.
7. Skills are first-class durable objects with provenance, version, status, target adapters, evidence refs, and validation status.
8. Install/sync is a separate policy-gated action from proposal/render.

## 4. Data contract additions / interpretation

Existing rows can model the first MVP:

- `sessions` and `turns`: raw session trace
- `file_changes`: file evidence
- `improvement_signals`: cheap/local signal layer
- `proposals`: review/render queue
- `skills`: canonical skill registry

For skill factory specifically:

```txt
improvement_signals.kind = skill_candidate
improvement_signals.source_ref = session:<session_id>
improvement_signals.summary includes raw_trace_ref=session:<session_id>
improvement_signals.metadata includes:
  - tool
  - project
  - turns
  - tool_calls
  - file_changes
  - candidate_reason
  - renderer: codex_gpt
```

When promoted:

```txt
proposals.kind = skill
proposals.status = triaged | staged | approved | applied | rejected
proposals.evidence_refs includes raw refs
proposals.body must say that renderer must replay raw refs before SKILL.md
```

## 5. AGENTS / worker implications

Every coding agent entering this repo must understand:

- Altevra has an internal model runtime.
- The internal runtime includes cheap_worker, strong_reasoner, local_private, embedding_model, and Codex/GPT renderer roles.
- Hermes/Claude/Codex are not just external users; they can be attached tools/adapters feeding and consuming Altevra.
- Do not describe skill factory as “Hermes will later ask Codex manually.” Correct framing: Altevra emits structured work for its attached Codex/GPT renderer.
- If docs or code imply that external model/API integration should not exist, treat that as stale P0-era guidance unless a current task explicitly scopes it out.

## 6. Vision

Altevra should become a compounding agent operating system:

- it records real work without losing detail
- it notices repeated workflows
- it turns repeated workflows into skills
- it syncs those skills into the right agents
- it observes whether those skills help
- it improves prompts/tools/skills safely over time

The point is not automation for its own sake. The point is that Pavle's system gets better every week because every real workflow leaves a reusable operational trace.

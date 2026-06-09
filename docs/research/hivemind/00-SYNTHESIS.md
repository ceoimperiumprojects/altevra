# Hivemind Deep Dive — Synthesis & Decision

**Date:** 2026-06-08
**Author:** Claude (Opus 4.8) + 6 parallel documentation agents
**Subject:** `activeloopai/hivemind` (`@deeplake/hivemind` v0.7.84, Apache-2.0)
**Why:** Pavle found Hivemind looks like Altevra's vision already built. Decision: document deeply, take the good ideas, then decide build-vs-use. This is the verdict.

Section docs: [01-skillify-engine](01-skillify-engine.md) · [02-storage-capture](02-storage-capture.md) · [03-knowledge-graph](03-knowledge-graph.md) · [04-integration-mcp-rules](04-integration-mcp-rules.md) · [05-proactive-goals-dashboard](05-proactive-goals-dashboard.md) · [06-propagation-sync-ops](06-propagation-sync-ops.md)

---

## TL;DR verdict: **BORROW, DON'T ADOPT.**

Hivemind is a **cloud-only SaaS** (Activeloop Deep Lake). It has **no local datastore** — `~/.deeplake/memory/` is a *virtual* filesystem synthesized from cloud rows, never written to disk; `loadConfig()` no-ops without a `token`+`orgId`; logged out, the product is inert (only offline `skillify mine-local` works). For a **local-first, single-user, sovereign** system (Altevra's founding axiom), Hivemind is **unusable as-is** — using it means sending Pavle's data to Activeloop's metered cloud.

But its *designs* are excellent and now fully documented (Apache-2.0). **Steal the engine, skip the cloud.**

This corrects the earlier read that Hivemind was "local-first with opt-in cloud like Altevra." It is not. It is cloud-first SaaS.

---

## What the README oversells (verified against code)

- **"Graph from edges your agents actually traverse"** → false. It's a pure **static tree-sitter AST** extraction over `git ls-files` (same as graphify); sessions only *trigger* rebuilds, they never feed a node/edge. Internal code admits "AST-based, NOT semantic."
- **"Trajectory export for fine-tuning"** → zero code in repo; it's just `SELECT` over Deep Lake's tensor backend, marketing-by-association.
- **"Real-time propagation"** → pulled at SessionStart, not into open sessions.
- **No embeddings/vector search in retrieval** of the main store — recall is plain SQL; embeddings (local `nomic-embed-text` 768d) exist but vectors are stored in the cloud.

It IS real and mature though: 223 test files, CI, YC/Activeloop-backed, v0.7.84 on npm. Not vaporware — just cloud-coupled and somewhat over-marketed.

---

## The crown jewel worth stealing: the SkillOpt optimizer

`src/skillify/skill-edits.ts` is an explicit port of a research "SkillOpt" algorithm — the **reflect→edit backward pass** for skills:

- **4 deterministic edit ops** (append / insert_after / replace / delete) — pure, unit-testable.
- **Edit budget = "textual learning rate"** (default 3): bounds how much a skill changes per improvement cycle.
- **Protected slow-update region** (`<!-- SLOW_UPDATE_START/END -->`): fast edits can't touch the stable core.
- **Proposer diagnoses the SINGLE recurring weakness** and proposes a small structured edit, not a rewrite.
- **Meta-skill memory** (JSONL, order-independent fingerprints): never re-tries an edit that already failed.
- **Anti-sycophancy success judge**: "was it CORRECT — ignore whether the user seemed happy"; unparseable/errored → treat as success so a flaky judge can't manufacture deficiency.
- Two engines: **SKILLIFY** (forward pass: mine sessions → Haiku KEEP/SKIP/MERGE gate → write SKILL.md) on Stop-counter(20)+SessionEnd; **SkillOpt** (backward pass: bad reaction to an invoked skill → improve) event-driven.
- Models routed through the **user's own agent CLI** (claude/codex/...), Haiku=judge, Sonnet=proposer.

This is more advanced than Altevra's S2 sketch and maps cleanly onto Altevra's `proposals`/trust-ladder model — **in Rust/SQLite, routed via `altevra-llm` roles, with the publish step gated by Altevra's review queue (invert their auto-publish).**

---

## Adoptable patterns for Altevra (the "uzeo sve dobro" list)

| From | Steal this | For Altevra |
|------|-----------|-------------|
| skillify | SkillOpt edit-ops + edit-budget + slow-update region + diagnose-one-weakness proposer + meta-fingerprint memory + anti-sycophancy judge | S2 skill factory (port to Rust, gate publish via trust ladder) |
| skillify | event-driven firing (PreToolUse(Skill)→reaction window) + oldest-watermark mining | when to mine candidates |
| storage | append-only version-bump audit trail; best-effort non-blocking embed; idle-exit embedding daemon; exit-0-surface-nothing hook safety | capture + S0 hook robustness |
| storage | local `nomic-embed-text` 768d (confirms S1 choice) | S1 embedding role |
| graph | NetworkX format, EXTRACTED/INFERRED/AMBIGUOUS confidence labels, content-hash identity, validate-before-write, reverse-BFS impact, VFS `cat` query surface | v0.6 knowledge graph |
| graph | **the gap neither fills:** embedding entity-resolution + traversal-weighted co-recall edges from real session history | Altevra's unique graph edge |
| integration | **SessionStart RULES/GOALS injection** (Altevra lacks this), per-tool channel awareness (user-visible vs model-visible), path-normalized merge | adapters + `altevra context` |
| proactive | Source/Rule/Delivery contract + atomic dedup + `userVisibleOnly` flag + high-precision-or-silent relevance gate | S4 daily briefing + relevance gate |
| proactive | path-encodes-structure for goals/KPIs (`goal/<owner>/<status>/<id>.md`) — as a SQLite column, not a real VFS | personal brain / goals |

## What to SKIP (cloud/team-coupled, against sovereignty)

Deep Lake cloud control plane, JSONB-discriminator single-table schema, string-concat SQL, `me|team`/org-publish/auto-pull/cross-author propagation, marketplace-scanner bundling, device-flow token in plaintext, metered SaaS billing.

---

## Decision & GTM alignment

1. **Don't adopt Hivemind** — cloud-only, violates Altevra's sovereignty axiom; a solo offline user gets nothing.
2. **Don't rebuild it in Rust now either.** The designs are captured in these 6 docs (the real prize). Building the Rust port is exactly the "stop building" anti-pattern.
3. **Park Altevra** with this documented design debt. When the skill factory IS built (post-GTM), these docs are the blueprint — and Altevra's local-first + personal-life + sovereign angle remains its undisputed moat (Hivemind does none of it).
4. **Push ReVesta now** (Đorđe's directive: 2 paid Simple Surplus clients). Knowledge captured, gotov alat understood, no more internal-tooling rabbit holes blocking revenue.

**One-line:** Hivemind validated the category and handed us a free, documented blueprint for the hardest part (the skill optimizer) — we took the knowledge, we don't pay the cloud tax, and we go sell.

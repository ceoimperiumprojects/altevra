# Hivemind SKILLIFY engine — deep dissection

> Research note for Altevra's skill-factory design.
> Source: `~/projekti/vendor/hivemind/src/skillify/` (Apache-2.0).
> Read-only study; no hivemind source was modified.
> All file refs are `path:line` relative to `src/skillify/`.

Hivemind's SKILLIFY engine is **two distinct sub-engines** that share a
vocabulary ("skill", "gate", "verdict") but are wired completely differently:

1. **SKILLIFY (the miner / crystallizer)** — watches recent agent sessions,
   asks an LLM "is there a recurring pattern worth a SKILL.md here?", and
   writes a fresh skill (or merges into an existing one). This is the *forward
   pass* — knowledge → skill.

2. **SkillOpt (the optimizer / backward pass)** — when an *already-published*
   org skill gets invoked and the user reacts badly, judge "did the task
   actually succeed?", and if not, propose a small bounded edit and republish
   `v+1` immediately. This is the *backward pass* — failure signal → edit.

The two are deliberately separate codepaths with separate triggers, separate
workers, separate state, and (mostly) separate models. The naming convention
that links everything: an org skill is identified by `name--author`
(`skill-invocations.ts:80` `splitOrgSkill`).

---

## 1. End-to-end pipeline

### 1.1 SKILLIFY (mine → judge → write) — the forward pass

```
                  agent Stop / SessionEnd hook
                              │
        ┌─────────────────────┴──────────────────────┐
        │ triggers.ts                                  │
        │  tryStopCounterTrigger  (counter ≥ N=20)     │
        │  forceSessionEndTrigger (always, on end)     │
        └─────────────────────┬──────────────────────┘
                              │ acquire per-project worker lock
                              ▼
        spawn-skillify-worker.ts  → writes config.json (0600) to tmp,
                                     spawns detached node worker
                              ▼
   ┌──────────────────────── skillify-worker.ts ───────────────────────┐
   │ 1. readState → lastDate watermark            (state.ts)            │
   │ 2. listCandidateSessions: newest 10 sessions in scope,            │
   │    author-filtered, after watermark, EXCLUDING current session    │
   │ 3. fetchSessionRows → extractPairs            (extractors/index.ts)│
   │    → strip tool_calls/thinking, pair USER↔ASSISTANT                │
   │ 4. buildPrompt: existing-skills block + recent exchanges          │
   │    + KEEP/SKIP/MERGE rules                                         │
   │ 5. runGate: shell out to the agent's OWN CLI (Haiku for claude)   │
   │    (gate-runner.ts)                                                │
   │ 6. parseVerdict from verdict.json file OR stdout (gate-parser.ts) │
   │ 7. KEEP → writeNewSkill ; MERGE → mergeSkill  (skill-writer.ts)   │
   │ 8. insertSkillRow → Deeplake `skills` table   (skills-table.ts)   │
   │ 9. advanceWatermark / recordSkill             (state.ts)          │
   └────────────────────────────────────────────────────────────────────┘
                              ▼
        Deeplake `skills` table (append-only, version DESC = current)
                              ▼
        SessionStart auto-pull on every teammate's machine
        (auto-pull.ts → pull.ts → renderSkillFile → ~/.claude/skills/<name>--<author>/SKILL.md)
```

**Trigger (`triggers.ts`).** Two firing paths, both non-blocking, both
recursion-guarded by `HIVEMIND_SKILLIFY_WORKER=1`:
- `tryStopCounterTrigger` (`triggers.ts:42`) bumps a per-project counter on
  every assistant-complete event; fires when `counter ≥ TRIGGER_THRESHOLD`
  (default **20** turns, env `HIVEMIND_SKILLIFY_EVERY_N_TURNS`, `state.ts:48`).
- `forceSessionEndTrigger` (`triggers.ts:84`) always fires on session end to
  catch tail-of-session knowledge (and the `claude -p` one-shot case where Stop
  never fires).
Both acquire a per-project worker lock (`state.ts:193 tryAcquireWorkerLock`,
10-min stale reclaim) so only one miner runs per project at a time.

**Mining (`skillify-worker.ts`).** Constants at `skillify-worker.ts:100-103`:
`SESSIONS_TO_MINE = 10`, `PAIR_CHAR_CAP = 2000`, `TOTAL_PAIRS_CHAR_CAP =
40_000`, `EXISTING_SKILLS_CHAR_CAP = 30_000`. It pulls candidate sessions from
the Deeplake `sessions` table (`listCandidateSessions`, `:192`), author-scoped
(`authorClause`, `:179`), newer than the watermark, **excluding the in-flight
session** (`isCurrentSession`, `:207`). Rows → `extractPairs` strips all
`tool_call` rows and pairs each `user_message` with the following
`assistant_message`(s) (`extractors/index.ts:47`). Thinking blocks never reach
Deeplake at all (capture skips them).

**The keep/no-keep judge.** This is a *single gate call* that decides KEEP /
SKIP / MERGE in one shot — there is no separate "worth keeping?" model before
the proposer. The prompt (`buildPrompt`, `skillify-worker.ts:265`) hard-codes
the keep bar:
> "KEEP only if the pattern recurs across at least **3 of the exchanges**, is
> **non-obvious** to a competent engineer, and is **not already covered** by an
> existing skill" (`:276`).

The verdict is parsed either from a `verdict.json` the model wrote via its Write
tool, or from stdout JSON as fallback (`readVerdict`, `:325`; `parseVerdict`,
`gate-parser.ts:40` does balanced-brace extraction). Unparseable → treated as
SKIP, watermark advanced, tmp kept for inspection (`:418`).

**Write / merge (`skill-writer.ts`).** `writeNewSkill` (`:194`) refuses to
overwrite (`existsSync` → throw), seeds `version: 1`, `author`,
`contributors: [author]`. `mergeSkill` (`:232`) reads existing frontmatter,
unions `source_sessions`, bumps version, appends the editor to `contributors`
(author is **immutable** across merges). `assertValidSkillName` (`:103`)
enforces strict kebab-case + rejects path traversal — the name comes from
untrusted model output / network rows.

**Watermark semantics — subtle and worth stealing.** The watermark is set to
the **OLDEST mined session**, not the newest (`skillify-worker.ts:376-384`,
`:429-435`). Reason: SQL orders DESC then `LIMIT 10`; if you set the watermark
to the newest, any session older than the LIMIT cutoff is *permanently* skipped.
Setting it to the oldest re-mines the recent N next run (benign — same input →
SKIP, no new row) but never loses an older session.

### 1.2 SkillOpt (judge → edit → republish) — the backward pass

```
   PreToolUse(Skill, ORG skill only)
        │  skillopt-trigger.ts:markSkillPending
        ▼  opens a K-message judgment window (default K=3) for X in this session
   UserPromptSubmit  (the user's "reaction")
        │  skillopt-trigger.ts:runEventTrigger → spawn detached worker, decrement budget
        ▼
   ┌──────────── skillopt-worker.ts ────────────┐
   │ detectScorerAgent → run on USER's own agent │
   │ per-skill lock (no double-publish)          │
   │ load meta.jsonl (cross-run edit memory)     │
   └──────────────────┬──────────────────────────┘
                      ▼
   ┌──────────── skillopt-improve.ts:improveSkillIfFailed ───────────┐
   │ 1. findInvocation (+retry for Deeplake insert lag)              │
   │ 2. windowAroundInvocation (3 before / 6 after, elided 4000ch)   │
   │    + append the just-submitted reaction from the hook payload   │
   │ 3. judgeSuccess(window)            → success 0|1 (success-judge) │
   │    success≠0 → STOP (no change)                                 │
   │ 4. readCurrentSkillRow from Deeplake skills table               │
   │ 5. proposeSkillEdit(body, [reason], {priorEdits})  ← the EDIT    │
   │    (skill-proposer.ts → skill-edits.ts bounded ops + budget)     │
   │ 6. alreadyProposed? (meta dedup) → STOP                         │
   │ 7. publishImprovedSkill: INSERT v+1, scope=team       (org-publish)│
   │ 8. recordEdit → append meta.jsonl (best-effort)                 │
   └──────────────────────────────────────────────────────────────────┘
```

Critically there is **NO gate / A/B validation between propose and publish in
the live SkillOpt path**. The proposer produces the edited body, the meta-dedup
guards against re-publishing the same edit, and `publishImprovedSkill` writes
`v+1` directly to the org table. The header comment is explicit:
> "No approval gate by design: detect → improve → publish, directly."
> (`skill-org-publish.ts:5`)

The "real-usage A/B gate" exists only as a *mechanism* in `skill-publisher.ts`
(`publishSkillEdit`, `:49`, with version bump + `.bak` backup), and its own
header says it is **deliberately not called** by the live worker, reserved for a
"deferred" gated path because "the offline gate isn't trustworthy"
(`skill-publisher.ts:8-11`). So in shipped behavior: SKILLIFY uses the LLM gate
to decide KEEP/SKIP/MERGE; SkillOpt uses the success-judge as its only gate and
publishes ungated.

---

## 2. The reflect → edit optimizer (the paper's backward pass)

This is the crown jewel and the most directly stealable piece. It is an explicit
port of "SkillOpt's skillopt/optimizer/skill.py" (`skill-edits.ts:3`).

### 2.1 Bounded structured edits — `skill-edits.ts`

Four edit ops only (`skill-edits.ts:12`):
`append | insert_after | replace | delete`. Each `Edit` carries an `op`, an
anchor `target` (exact existing substring), and `content`. `applyEdits`
(`:52`) is **pure, deterministic, no I/O** — fully unit-testable. Anchors are
matched by `indexOf` (exact substring), so a hallucinated anchor that isn't
present is simply skipped with `SKIP ... target not found` (`:78`,`:90`,`:99`)
rather than corrupting the doc.

### 2.2 Edit budget = "textual learning rate" — `skill-edits.ts:41`

```ts
export function selectEdits(edits: Edit[], budget: number): Edit[] {
  return edits.slice(0, Math.max(0, budget));
}
```
Default budget = **3** (`skill-proposer.ts:80`). This is the optimizer's
learning-rate knob: a large LR (rewrite everything) overfits to the single
failure; a small LR (≤3 surgical edits) nudges the doc. The proposer's system
prompt reinforces it: "propose a SMALL set", "Do NOT rewrite the whole doc",
"Prefer the smallest change that fixes the weakness" (`skill-proposer.ts:28-36`).

### 2.3 Protected slow-update region — `SU_START` / `SU_END`

`skill-edits.ts:19-20`:
```
SU_START = "<!-- SLOW_UPDATE_START -->"
SU_END   = "<!-- SLOW_UPDATE_END -->"
```
A region of the SKILL.md holding *longitudinal guidance* that fast per-edit
changes must never touch (the paper's "slow update" vs "fast update" split).
`targetsProtected` (`:29`) rejects any edit whose `[idx, idx+len)` range
**overlaps** the protected block at all — not just edits that start inside it,
so an anchor that begins just before `SU_START` and spans into it cannot delete
protected guidance (`:36`). `append` lands *above* the protected block
(`:67-68`) so freshly appended fast-updates never push into the slow region. The
proposer's system prompt also tells the model the region is off-limits
(`skill-proposer.ts:31`), so it's defended both in the prompt and in the
deterministic apply step.

### 2.4 Proposer diagnoses ONE weakness — `skill-proposer.ts`

`proposeSkillEdit` (`:75`) is the "reflect" step. It feeds the model:
- the current skill body,
- up to 8 **confirmed failures** (the success-judge's reasons),
- and (meta-skill) the list of edits already tried for this skill.

System prompt (`:28`): "Diagnose the **SINGLE recurring weakness** behind the
failures and propose a SMALL set of structured edits that fix it." Output is a
JSON array of edits, tolerantly parsed (`parseEdits`, `:49`, strips ``` fences
and surrounding prose). The model call is **injected** (`ModelCall`) so the
whole reflect logic is unit-testable with zero real LLM calls (`:81` default =
`claudeModel("sonnet")`).

### 2.5 Meta-skill — don't repeat tried edits — `skillopt-meta.ts`

The optimizer's cross-run memory. Append-only JSONL at
`<stateDir>/skillopt/meta.jsonl`. Each `MetaEntry` (`:17`) records the skill
ref, per-edit summaries (`ops`), an **order-independent fingerprint** of the
edit set (`fingerprintEdits`, `:35` — sort + join so the same edits dedup
regardless of order), timestamp, and a `status` (`proposed → applied/reverted`).
Two functions close the loop:
- `alreadyProposed` (`:63`): has this exact edit set been tried for this skill?
  If yes, the worker stops before publishing (`skillopt-improve.ts:141`).
- `priorEditSummaries` (`:70`): feeds "what's been tried" back into the
  proposer prompt so it proposes *something different, or nothing*
  (`skill-proposer.ts:40-42`).

This is what prevents the loop from churning the same edit forever each time a
re-judged window re-fires.

---

## 3. Trigger optimization, success judging, gates

### 3.1 Event-driven trigger — `skillopt-trigger.ts`

SkillOpt firing is **not** time-throttled (the old weekly SessionStart throttle
was replaced). It fires on **real bad-skill signal** only:
- `markSkillPending` (`:102`) is called from `PreToolUse` when an **org** skill
  is invoked (must pass `splitOrgSkill` shape AND be in the pull manifest —
  `defaultIsOrgSkill`, `:53`, so a local skill can't shadow an org row). It
  opens a K-message window (default `DEFAULT_JUDGE_WINDOW = 3`, `:39`, env
  `HIVEMIND_SKILLOPT_JUDGE_WINDOW`). The pending state is stored **per-session
  in its own file** (`pendingFile`, `:63`) so two concurrent sessions can't
  clobber each other's pending entry.
- `runEventTrigger` (`:127`) fires on each `UserPromptSubmit` (the "reaction"),
  decrements the window budget, closes the window when spent, and spawns the
  detached worker (`spawnWorker`, `:151`) passing session/skill/reaction/
  toolUseId via env (`skillopt-env.ts`). A session that never invokes an org
  skill never opens a window → zero overhead. Kill switch:
  `HIVEMIND_SKILLOPT_DISABLED=1`.

The `toolUseId` is captured at `PreToolUse` and used to **pin the exact
invocation window** to judge, so a quick re-invocation of the same skill before
the worker queries can't make it judge the wrong window
(`skillopt-improve.ts:findInvocation:37`).

> Note: "trigger optimization" here means *when the loop fires* (event-driven on
> a confirmed bad reaction), not an LLM that rewrites a skill's `trigger:`
> frontmatter field. There is no separate `skillopt-trigger`-model that tunes
> trigger phrasing; trigger text is just carried through as a frontmatter field
> (`skill-writer.ts:131`) and improved only if the proposer happens to edit it.

### 3.2 Success judge — the anti-sycophancy gate — `success-judge.ts`

`judgeSuccess` (`:60`) asks the ONE question that resists sycophancy: *was the
task accomplished CORRECTLY?* — explicitly "Ignore whether the user seemed happy
or polite — a praised-but-wrong answer is a FAILURE" (`:30`). Returns
`{success: 0|1, confidence, reason}`. Three defensive properties:
- **Conservative on failure** (`:51-57`, `:65`): an unparseable / errored /
  empty judgment returns `success=1` (NOT a failure). A flaky judge can only
  fail to detect deficiency; it can never *manufacture* it. The next run catches
  what this one missed.
- **Injected model** (`:60`) — unit-testable with no LLM.
- **Cheap default** — `claudeModel("haiku")` (`:62`), because it only ever runs
  on anchor-flagged windows, never on every session.

### 3.3 What "gates" actually exist

| Gate | File | Purpose | Model |
|------|------|---------|-------|
| **KEEP/SKIP/MERGE gate** | `gate-runner.ts` + `gate-parser.ts` | SKILLIFY's decision to crystallize a pattern into a skill | Haiku (claude) |
| **Success judge** | `success-judge.ts` | SkillOpt's "did the task fail?" gate before any edit | Haiku default |
| **Advisor (executor/advisor)** | `advisor.ts` | Picks the single best insight from N mine-local candidates | Sonnet |
| **A/B publish gate** | `skill-publisher.ts` | version-bump + `.bak` mechanism — **built but NOT wired into the live loop** ("deferred") | n/a |

`gate-runner.ts:runGate` (`:190`) shells out to the originating agent's *own*
CLI with bypass/yolo flags and tool-write enabled (the gate needs the Write tool
for `verdict.json`). It is the only LLM call in the engine that runs *with*
tools; the judge/proposer/advisor all run tool-free (untrusted transcript text).

---

## 4. Scope, promotion, pull/publish/org-publish

### 4.1 Scope model — `scope-config.ts`

Only two scopes survive: `me | team` (`:23`). Persisted at
`~/.deeplake/state/skillify/config.json` as `{scope, team[], install}`. `install`
is `project` (→ `<cwd>/.claude/skills`) or `global` (→ `~/.claude/skills`),
`skill-writer.ts:297 resolveSkillsRoot`. A legacy `"org"` scope is silently
coerced to `"team"` on read (`:56`). Author filter:
- `scope=me` → mine only my own sessions (`authorClause`, `skillify-worker.ts:189`)
- `scope=team` + team list → mine sessions authored by anyone in the list (`:185`)

### 4.2 Cross-author MERGE auto-promotion — `scope-promotion.ts`

When the editor of a MERGE is **not** the original author, the Deeplake row's
scope is bumped `me → team` (one-directional, never `team → me`). Pure helpers
`isCrossAuthorMergeVerdict` (`:22`) and `resolveRecordScope` (`:34`) pin the
policy so it's unit-testable. KEEP and same-author MERGE never promote. The
worker threads this at `skillify-worker.ts:477-485`. Provenance: `author` is the
immutable v=1 creator; `contributors[]` grows chronologically.

### 4.3 Publish back to org — `skill-org-publish.ts`

`readCurrentSkillRow` (`:55`) reads the latest version
(`ORDER BY version DESC, created_at DESC LIMIT 1` — deterministic tie-break for
cross-machine races). `publishImprovedSkill` (`:108`) inserts `v+1`,
**append-only** (never UPDATE — avoids the "two rapid UPDATEs drop one"
Deeplake quirk, `skills-table.ts:14`), sets `scope=team`, appends the triggering
user + a `"skillopt"` contributor marker (`SKILLOPT_CONTRIBUTOR`, `:19`), keeps
`name`/`author` unchanged.

### 4.4 Pull / auto-pull — `pull.ts` + `auto-pull.ts`

`runPull` (`pull.ts:456`) queries the `skills` table, keeps the highest version
per `(project_key, name)` (`selectLatestPerName`, `:319`), and writes
`<root>/<name>--<author>/SKILL.md` (`renderSkillFile`, `:338`). `decideAction`
(`:432`): write if local missing OR remote newer OR `--force`; else skip.
Version is the conflict-resolution unit. Pulled skills fan out as **symlinks**
into every detected non-Claude agent root (`fanOutSymlinks`, `:216`;
`backfillSymlinks`, `:282` covers the "installed a new agent after pulling"
case). A `pulled.json` manifest (`manifest.ts`) is the authority for what
`unpull` may remove (so it never deletes a user-authored `name--author` skill).

`autoPullSkills` (`auto-pull.ts:75`) runs on **every SessionStart**, no
throttle (writes are idempotent), 5s timeout, all failures swallowed, opt-out
`HIVEMIND_AUTOPULL_DISABLED=1`. Equivalent to
`hivemind skillify pull --all-users --to global`. This is the propagation flywheel:
teammate mines a skill at 10:01, you get it when you open a session at 10:02.

---

## 5. Which LLM does what, and is any of it local?

| Step | Default model | Where | Notes |
|------|--------------|-------|-------|
| SKILLIFY KEEP/SKIP/MERGE gate | **Haiku** | `gate-runner.ts:155` (`--model haiku`) | runs with Write tool; bypass perms |
| SkillOpt success judge | **Haiku** | `success-judge.ts:62`; `agent-model.ts:50` (`role==="judge" → "haiku"`) | tool-free; conservative-on-fail |
| SkillOpt proposer (the edit) | **Sonnet** | `skill-proposer.ts:81`; `agent-model.ts:50` (`role==="proposer" → "sonnet"`) | tool-free |
| Mine-local executor (candidate gen) | **Haiku** ×N parallel | `advisor.ts:13` | cheap fan-out |
| Mine-local advisor (pick best) | **Sonnet** ×1 | `advisor.ts:127` (`--model sonnet`) | executor/advisor pattern |

So the split is exactly **Haiku = cheap judge/gate, Sonnet = capable
proposer/advisor**.

**Multi-agent, not multi-provider-local.** The clever part: the scorer runs on
the **user's own agent CLI**, whatever that is — claude / codex / cursor-agent /
hermes / pi (`agent-model.ts:40 DISPATCH`, `detectScorerAgent:177`). Cost lands
on the user, and a codex/hermes user with no `claude` installed still gets
SkillOpt. Each agent runs in its **safest tool-free mode** (claude `--tools ""
--strict-mcp-config`, codex `-s read-only`, etc., `agent-model.ts:44-99`)
because the prompt contains untrusted transcript text.

**Is any of it local-as-in-on-device?** Not really. The "models" are always a
hosted frontier model reached *through* a local CLI. Hermes/pi can point at any
provider via `--provider`/`-m` (OpenRouter, Bedrock, Google — `gate-runner.ts:172-186`,
`agent-model.ts:63-99`), and *in principle* could point at Ollama/vLLM, but
nothing in skillify assumes or optimizes for a local model. There is **no
embedding model, no vector search, no reranker** anywhere in the skillify
engine — retrieval is pure SQL over Deeplake (`LIKE`, `ORDER BY ... LIMIT`).

---

## 6. Adoptable for Altevra

Altevra's vision (CLAUDE.md §12) already calls for a generalized self-improving
skill factory that proposes skills for Claude Code / Codex / Cursor / Hermes,
with a trust ladder (auto-apply vs review). Hivemind's SKILLIFY is the closest
existing implementation of exactly this. Here's the steal/skip breakdown.

### 6.1 STEAL — directly portable to local-first single-user Rust

1. **The bounded-edit optimizer (`skill-edits.ts`).** This is the single best
   idea. Four ops + edit budget + protected slow-update region is a clean,
   *deterministic, pure-function* design that ports to Rust almost
   line-for-line and is trivially unit-testable. Altevra should adopt:
   - `append / insert_after / replace / delete` with exact-substring anchors,
   - `edit_budget` (default 3) as the "textual learning rate",
   - `<!-- SLOW_UPDATE_START/END -->` protected region for longitudinal guidance
     that fast edits can't touch (maps perfectly onto Altevra's "version history
     preserved" + "preferences refined over time" doctrine).

2. **The reflect → diagnose-ONE-weakness → small-edit loop
   (`skill-proposer.ts`).** Feed the proposer (a) the skill body, (b) confirmed
   failures, (c) prior tried edits, and ask for *the single recurring weakness*.
   The injected-`ModelCall` design (test with zero LLM calls) is exactly what
   Altevra wants given its local-model story.

3. **The anti-sycophancy success judge (`success-judge.ts`).** "Was the task
   done CORRECTLY, ignore whether the user was happy" + **conservative-on-
   failure** (unparseable → success=1, never manufacture deficiency). This is a
   genuinely subtle reward-signal design that Altevra's observer/insight modes
   should copy verbatim. Pairs with Altevra's relevance-gate philosophy (signal,
   not noise).

4. **The meta-skill / edit memory (`skillopt-meta.ts`).** Append-only JSONL +
   order-independent fingerprint + "don't repeat tried edits" + feed priors back
   into the proposer. This is what stops the loop from churning. In Altevra this
   maps to a small SQLite table (`skill_edits(skill_id, fingerprint, ops_json,
   status, proposed_at)`) — even cleaner than JSONL given Altevra is already
   SQLite-first.

5. **Event-driven firing on confirmed bad signal (`skillopt-trigger.ts`).** Open
   a K-message window when a skill is invoked; only spend LLM budget when the
   user actually reacts; pin the exact invocation by tool_use_id. This is far
   better than time-throttled sweeps and fits Altevra's "proactive but not
   noisy" mandate. The per-session-file pending store avoids a shared-map race —
   in Altevra, a row per session keyed by session_id.

6. **The watermark = oldest-mined, not newest trick
   (`skillify-worker.ts:376-384`).** Subtle correctness fix worth internalizing
   for *any* "process newest N, advance cursor" loop, including Altevra's
   session-import flywheel. Re-processing is benign (→ SKIP); skipping an old
   record is permanent data loss.

7. **Append-only versioning + `version DESC LIMIT 1` (`skills-table.ts`,
   `skill-org-publish.ts`).** Never UPDATE a skill row; insert a new version.
   Gives free history, free rollback, and dodges write-coalescing races.
   Altevra already wants versioned identity history — same pattern.

8. **Executor/advisor for candidate selection (`advisor.ts`).** Cheap model
   produces many candidates, one capable model picks the best with a strict
   GOOD/BAD insight rubric. Maps onto Altevra's `cheap_worker` + `strong_reasoner`
   model-routing roles directly, and onto "surface the best to the daily digest".

9. **Strict input validation on model-derived names (`skill-writer.ts:103
   assertValidSkillName`, `skill-invocations.ts:80 splitOrgSkill`).** Skill
   names/authors become filesystem paths and SQL literals from untrusted LLM
   output. Altevra must do the same kebab-case + no-traversal + SQL-escape
   discipline (Rust: a `SkillName(String)` newtype with a validating
   constructor).

10. **Tool-free scorer prompts.** Judge/proposer always run with tools disabled
    because the prompt embeds untrusted transcript text. Altevra's local skill
    factory must enforce the same: classification/diagnosis LLM calls get **no
    tools**, no MCP, no shell.

### 6.2 SKIP / heavily adapt — cloud/team-coupled, not for local-first single-user

1. **Deeplake as the store + all `skills`/`sessions` table SQL.** The entire
   read/write substrate is a hosted multi-tenant Deeplake API
   (`skillify-worker.ts:125 query`, `pull.ts`, `skills-table.ts`). Altevra is
   local SQLite-first by axiom — replace with local tables. The *shape*
   (append-only rows, `version DESC`) transfers; the transport does not.

2. **`me | team` scope + cross-author MERGE auto-promotion + org-publish
   (`scope-config.ts`, `scope-promotion.ts`, `skill-org-publish.ts`).** This is
   the entire multi-user collaboration layer. Altevra is single-user (Pavle).
   `author`/`contributors` lineage is pointless for one person; scope is always
   "me". Drop the promotion policy, the team author-filter, and org-publish
   entirely. Keep only the local write path.

3. **SessionStart auto-pull + symlink fan-out (`auto-pull.ts`, `pull.ts
   fanOutSymlinks/backfillSymlinks`, `manifest.ts`).** This whole machinery
   exists to propagate teammates' skills across machines and across multiple
   agent install roots. For a single user on one machine it collapses to "write
   the file once." Altevra *does* want multi-agent fan-out (write a skill into
   Claude Code AND Codex AND Cursor dirs — CLAUDE.md §12 "skill manufacturing
   layer"), so the **`fanOutSymlinks` + manifest idea is worth keeping in
   reduced form** (symlink/copy one canonical skill into each local agent's
   skills dir, manifest to know what Altevra wrote vs the user) — but the
   cross-machine/org-version reconciliation is not needed.

4. **The ungated detect→improve→publish path
   (`skill-org-publish.ts:5`).** Hivemind publishes `v+1` with no human review
   because shared org velocity is the goal and the offline A/B gate "isn't
   trustworthy." Altevra's doctrine is the **opposite** — a trust ladder where
   prompt tweaks / source-of-truth edits / sensitive pages require Pavle's
   approval (CLAUDE.md §12). So Altevra should adopt the *opposite default*: the
   proposer's `editedBody` goes to a **review item** (Altevra already has
   `create_review_item` / `propose_improvement` MCP tools), and only
   low-risk/auto-tier edits apply unattended. The `skill-publisher.ts` version-
   bump + `.bak` mechanism is actually the *right* primitive for Altevra's gated
   path (it's the one hivemind built but didn't wire up).

5. **The static-scanner / ClawHub bundling contortions
   (`gate-runner.ts:22-41`, `:74-95`, the `createRequire` exec bypass and
   no-`process.env` path discovery, plus the openclaw `tuning` globalThis dance
   at `skillify-worker.ts:72-95`).** These are pure artifacts of shipping a JS
   bundle through a marketplace security scanner. Irrelevant to a native Rust
   binary — ignore entirely.

6. **Per-agent CLI shell-out for the model call (`gate-runner.ts`,
   `agent-model.ts`, `claude-model.ts`).** Hivemind has no API client of its own
   for scoring; it shells out to whatever agent CLI the user has so cost lands on
   the user. Altevra already has `altevra-llm` (multi-provider, native) — it
   should call its own model-routing layer (`cheap_worker`/`strong_reasoner`
   roles) directly, **not** shell out to `claude -p`. Keep the *role mapping*
   (cheap judge / capable proposer), drop the subprocess mechanism.

### 6.3 One-paragraph recommendation

Altevra should port `skill-edits.ts` (bounded ops + budget + slow-update region)
and `skillopt-meta.ts` (edit fingerprint memory) almost verbatim into Rust as
the core of its skill factory, wrap them with a `success-judge`-style
anti-sycophancy reward signal and a `proposer` that diagnoses one weakness, fire
the loop event-driven on confirmed bad reactions (not on a timer), store
everything append-only in local SQLite with `version DESC` semantics, and route
the judge/proposer through `altevra-llm`'s `cheap_worker`/`strong_reasoner`
roles instead of shelling out. Then **invert hivemind's publish policy**: route
the proposed edit to a review item by default and gate live-apply behind
Altevra's trust ladder, reusing the `skill-publisher.ts` version-bump+`.bak`
primitive. Drop Deeplake, drop the `me|team`/org-publish/auto-pull/cross-author
collaboration layer (single-user), and keep a reduced `fanOutSymlinks`+manifest
to write each canonical skill into every local agent's skills dir.

# Hivemind — Proactive + Productivity Layer (Notifications · Goals/KPIs · Dashboard)

Deep-dive into the *proactive* and *productivity* surfaces of Hivemind
(Apache-2.0, Activeloop/Deeplake) at `/home/pavle/projekti/vendor/hivemind/`.
All file refs are absolute-from-repo-root unless noted. Read-only audit.

Scope:
1. Notifications — sources, rules, delivery, proactive surfacing patterns
2. Goals + KPIs — VFS path-encodes-structure model, progress tracking
3. Dashboard — what it shows, how it's served
4. Rules / success-judge / advisor proactive intelligence
5. **Adoptable for Altevra** — what maps to S4 Daily Briefing + proactive notifications

---

## 0. Mental model in one paragraph

Hivemind's proactive layer is a **pull-on-session-start banner system** plus a
**push queue**, both draining into a single per-agent delivery adapter. There is
**no daemon, no cron, no background poller** — the *only* trigger today is the
Claude Code `SessionStart` hook. "Proactive" here means: *at the moment a new
session opens, decide the single most valuable thing to tell the user, plus any
queued/backend items, dedup it, and inject it into both the user terminal and
(selectively) the model context.* Goals/KPIs are a separate productivity store
backed by a **virtual filesystem where the path encodes the data** (owner,
status, ids), and the dashboard is a static HTML render served on localhost.

---

## 1. Notifications

### 1.1 Architecture overview

The framework is deliberately *trigger-agnostic* even though only `session_start`
is wired (`src/notifications/types.ts:13`, `Trigger = "session_start" | "ad_hoc"`).
Two ingress paths feed one drain:

- **Pull (rules):** pure functions evaluated each drain — `registerRule` /
  `evaluateRules` in `src/notifications/rules/registry.ts:14,25`.
- **Push (queue):** `enqueueNotification` writes to a persistent JSON queue
  (`src/notifications/index.ts:31`, re-exported from `queue.ts`), drained at the
  next session start.

The orchestrator is `drainSessionStart()` (`src/notifications/index.ts:77`). Its
pipeline (lines 77–152):

1. Read dedup `state` + `queue` (`readState`, `readQueue`).
2. Build a `NotificationContext` (creds, state, localSkillsCount, latestInsightEntry, sessionCount).
3. `evaluateRules("session_start", ctx)` → rule notifications.
4. Two parallel best-effort fetches under independent ~1.5s timeouts
   (`Promise.all`, line 100): `fetchBackendNotifications(creds)` and
   `pickPrimaryBanner(sessionId, creds, source)`.
5. Concatenate in reading order: **primary banner first**, then rules, then queue,
   then backend (line 107).
6. `alreadyShown` dedup filter (line 109) → `tryClaim` atomic per-notification
   claim (line 119, `O_CREAT|O_EXCL` claim file) to survive the two parallel
   SessionStart hook registrations (settings.json + marketplace hooks.json both fire).
7. `emit(agent, claimed)` (line 129).
8. Persist: non-transient → `markShown`; transient → `releaseClaim` (lines 137–142).
9. Drain queue unconditionally (line 146).

Everything is wrapped in a try/catch that **only logs, never throws** (line 149) —
a broken notification pipeline must never abort the session-start hook.

`Notification` shape (`src/notifications/types.ts:15`): `id`, `severity`,
`title` (≤80 chars), `body` (1–3 lines), `dedupKey` (object → JSON identity),
plus two important flags:
- `transient` (line 40) — show but **don't** record in `state.shown`; the enqueue
  itself is the rate limit (e.g. a recurring 402 error that self-clears on resolution).
- `userVisibleOnly` (line 59) — deliver to the **user terminal only**, suppress
  from model context. Guards against prompt-injection from LLM-derived prose.

### 1.2 SOURCES — what generates notifications

All under `src/notifications/sources/`. Each is fail-soft (returns `null`/`[]` on
any error so the hook keeps moving).

| Source | File | What it generates | Trigger condition |
|---|---|---|---|
| **Primary banner** | `primary-banner.ts:108` | The ONE top banner: welcome / savings recap / signup brief. Composes in the resume brief + open-goals summary. | Always picks exactly one (priority-ranked). Returns null on resume / no creds+brief / no sessionId. |
| **Resume brief** | `resume-brief.ts:269` | "Picking up on `<project>` — where you left off" with up to 2 sessions' open work + `/resume <sid>` + relative age. | Signed in + current project has captured summaries with non-empty `## Next Steps`. |
| **Cold-start brief** | `cold-start-brief.ts:374` | First-contact onboarding/conversion banner mined from local `~/.claude/projects/*.jsonl`. | First run (signed-in: once ever; anonymous: re-nudge ≤ once/24h until sign-in). |
| **Open goals** | `open-goals.ts:42` | One-line "N goals open:" + up to 3 sampled goal labels, appended to the primary banner body. | Signed in + ≥1 open goal in `hivemind_goals`. |
| **Org stats** | `org-stats.ts:162` | Not a notification itself — supplies org/user aggregates (sessions, recalls, bytes saved, balance cents) used to render savings recap + low-balance warning. | Cached 1h at `~/.deeplake/hivemind-stats-cache.json`; stale-on-failure fallback. |
| **Backend** | `backend.ts:82` | Server-pushed notifications from `GET /me/notifications` (e.g. billing top-up nudges). | Has token; 1.5s timeout; all malformed entries dropped. Always `userVisibleOnly`. |

#### Primary-banner priority logic (`primary-banner.ts:108–197`)
1. `!sessionId` → null (can't dedup the two parallel hook fires).
2. `source === "resume"` → null (user already has the thread; banner = noise).
3. `!creds.token` → **anonymous signup brief** (cold-start mined insight + `→ hivemind login`), `userVisibleOnly`.
4. Else fetch org stats → compute `tokensSaved` (`bytesToSavedTokens`, the LoCoMo 1.7× ratio, `BYTES_PER_TOKEN = 4`).
5. Fetch open-goals summary (best-effort).
6. Pick **session brief**: first signed-in session → cold-start brief; later → resume brief (exactly one fires, cold-start writes first-run state).
7. If `tokensSaved > MEANINGFUL_SAVINGS_TOKENS` (1,000, line 60) → **savings recap** (online or offline render); else → **welcome**.
8. `appendBalance` merges a soft low-balance warning if `0 < balanceCents < 200` (line 227).

`composeBody` (line 208) assembles **lead line → brief → 📌 goals**, blank-line separated.

### 1.3 RULES — when to fire (pure, IO-free)

Rules live in `src/notifications/rules/`, are registered at hook-load time, and
must be **pure** (`Rule.evaluate` contract, `types.ts:93`). Two shipped rules:

- **`local-mined-surfaced`** (`rules/local-mined.ts:46`) — surfaces locally-mined
  skills to *not-signed-in* users. Two branches: (1) a manifest entry with a
  concrete `insight` → "found a pattern in your past sessions" + minted skill name
  + login CTA, `userVisibleOnly` because the insight is LLM-derived; (2) fallback
  "🎉 N skills mined". Dedup keys on `skill_name + created_at` (insight) or `count`.
  Silent once creds present. `MAX_INSIGHT_CHARS = 90` with word-boundary truncation
  (line 35).

- **`referral-invite`** (`rules/referral-invite.ts:24`) — one-time "invite a
  teammate, org earns $20" banner. **Cadence gate**: `sessionCount >= MIN_SESSIONS`
  (3, line 22) so brand-new users aren't nudged; stable `dedupKey {v:1}` → shows
  once ever (bump version to re-nudge everyone). Silent when not signed in.

The registry (`rules/registry.ts`) is a flat array; `evaluateRules` filters by
trigger and collects non-null results. Duplicate rule ids throw at registration.

### 1.4 DELIVERY — channels

`src/notifications/delivery/` — a per-agent dispatch map. Today **only Claude Code**
has an adapter (`delivery/index.ts:29`, `ADAPTERS: Record<Agent, EmitFn>`).
`emit()` short-circuits on empty (line 33).

`emitClaudeCode` (`delivery/claude-code.ts:36`) implements the **dual-channel split**
(the codex P1 prompt-injection fix):
- **User-visible channel** = top-level `systemMessage` (renders as
  `SessionStart:startup says:` in the terminal). Gets **all** notifications.
- **Model-visible channel** = `hookSpecificOutput.additionalContext`
  (renders as a `<system-reminder>` to the model). Gets **only** notifications
  where `!userVisibleOnly` (line 42). Omitted entirely when nothing is model-safe
  (line 52).

Rendering (`format.ts:31`): severity-emoji prefix (`🐝` info / `⚠️` warn /
`🚨` error, line 20) + `title\nbody`, joined by blank lines. A project-convention
anti-pattern guard forbids the renderer template itself from embedding the strings
"DEEPLAKE MEMORY"/"HIVEMIND" (those belong to the sibling memory block).

`AGENT_CHANNELS.md` (in the notifications dir) is the forward-reference research on
each agent's harness behavior for future adapters (Codex, Cursor, Hermes, Pi, openclaw).

### 1.5 Proactive surfacing patterns ("X needs review", "pattern detected")

These are the *narrative* patterns Hivemind has actually shipped — worth copying:

- **"Pattern detected in your past sessions"** — `local-mined.ts:70`. A concrete,
  quantified insight mined from the user's own history + the artifact created to
  catch it next time + a one-line action. This is the canonical "the brain noticed
  something" surface.
- **"Where you left off"** — `resume-brief.ts:349`. Pulls the most recent
  *unfinished* work (`## Next Steps` section of captured summaries) and renders it
  as resumable pointers. High-precision-or-silent: never surfaces a stale TODO, never
  says "nothing pending", excludes still-live sessions (`excludeActiveSessions`, line 182).
- **"You tried to build your own recall / abandoned thread"** — `cold-start-brief.ts`
  signal extraction (`pickSignal`, line 286): recall-seeking openers (regex
  `RECALL_RE`, line 81), abandoned threads (`ABANDON_RE`, line 96), dominant-project
  volume. **High-precision-or-silent** — returns `quiet` if nothing clears the bar.
- **Cadence-gated nudges** — `referral-invite.ts` (wait 3 sessions), cold-start
  anonymous re-nudge (`RENUDGE_MS = 24h`, line 103). The brain doesn't nag every session.
- **Threshold-gated celebration** — savings recap only fires past 1,000 tokens saved.

Key engineering invariant across all of them: **fail-soft + bounded latency**
(every fetch has a hard timeout: backend/org-stats 1.5s, resume-brief 4s, cold-start
3.5s internal budget) so the proactive layer can never stall or break session start.

---

## 2. Goals + KPIs — the VFS path-encodes-structure model

### 2.1 The core design idea

**The filesystem path IS the schema.** The agent operates on a normal-looking
mount (`~/.deeplake/memory/`), but a path classifier intercepts goal/KPI paths and
routes reads/writes to dedicated tables. Path conventions
(`src/shell/goal-paths.ts:16,17`):

```
/memory/goal/<owner>/<status>/<goal_id>.md      → hivemind_goals row
/memory/kpi/<goal_id>/<kpi_id>.md               → hivemind_kpis row
```

- `status ∈ {opened, in_progress, closed}` (`goal-paths.ts:30`).
- `owner` = userName/email the agent reports.
- `goal_id` = UUIDv4 the agent generates at create time; `kpi_id` = short slug (`k-prs`).

The decisive design property (stated verbatim in `deeplake-schema.ts:122`): **path
decomposition is the source of truth for `owner`, `status`, `goal_id`** — the row's
`content` column stores ONLY the descriptive markdown body. There is *nothing to
drift* because the content does not replicate path-encoded fields. This avoids the
"path vs content drift footgun."

### 2.2 The classifier + compose/decompose helpers

`src/shell/goal-paths.ts`:
- `classifyPath(p)` → `"goal" | "kpi" | "memory"` (line 86). Minimal validation
  needed to dispatch; anything malformed falls through to plain `"memory"`.
- `segmentsUnderMemory(p)` (line 63) — robustly strips the mount prefix by finding
  the **last** `/memory/` occurrence, so it handles Write-tool mount-relative paths,
  `.deeplake/memory/`, and host-absolute `/home/<u>/.deeplake/memory/` forms (all
  arrive from different agents/shells).
- `decomposeGoalPath` (line 111) / `decomposeKpiPath` (line 135) — throw on
  malformed; extract structural parts.
- `composeGoalPath` (line 155) / `composeKpiPath` (line 163) — build the canonical
  VFS-internal path (no mount prefix) — the form deeplake-fs caches and DB rows store.

### 2.3 VFS routing (where the magic dispatches)

`src/shell/deeplake-fs.ts` imports the classifier (lines 16–22) and on bootstrap
**synthesizes the VFS tree from the tables**: it `SELECT`s goal rows and rebuilds
their canonical paths via `composeGoalPath({owner, status, goal_id})` (line 301),
same for KPIs (lines 317+). So `ls /goal/...` shows what's in the table.

On write, the VFS classifies the path; goal/kpi paths route to
`hivemind_goals`/`hivemind_kpis` instead of the generic memory table (lines 237–238,
gated on `fs.goalsTable`/`fs.kpisTable` being configured — degrades gracefully to
plain memory in test/legacy configs). There's also legacy handling for
pre-routing (`<=0.7.4`) rows that landed in the generic memory table (line 228+).

**Status transitions are filesystem operations**:
- Create goal = Write file at `goal/<owner>/opened/<uuid>.md`
- Move status = `mv goal/<u>/opened/<id>.md goal/<u>/in_progress/<id>.md` (atomic UPDATE)
- Soft-close = `rm goal/<u>/<status>/<id>.md` → VFS interprets `rm` as a status-flip
  to `closed` (no hard delete; row stays for audit)

(Documented in the agent instructions, `src/hooks/shared/goals-instructions.ts:38–43`.)

### 2.4 Schema (`src/deeplake-schema.ts`)

`GOALS_COLUMNS` (line 136): `id, goal_id, owner, status, content, version,
created_at, updated_at, agent, plugin_version`.
`KPIS_COLUMNS` (line 165): `id, goal_id, kpi_id, content, version, created_at,
updated_at, agent, plugin_version`.

Both are **immutable + version-bumped** (every write produces `v=N+1`; same pattern
as skills/rules). Read = latest version per id. This sidesteps a Deeplake
UPDATE-coalescing quirk and gives a full audit trail. KPI rows intentionally do
**not** store `owner` (line 157) — it's derived from the parent goal via logical
join on `goal_id`, so reassigning a goal owner doesn't cascade-move KPI files.
No FK enforcement on Deeplake; the join is purely logical.

### 2.5 Progress tracking

KPI body convention (`goals-instructions.ts:33`):
```
<KPI name>

- target: <int>
- current: <int>
- unit: <string>
```

Progress = mutate the `current:` line. Two paths:
- **VFS** (claude-code/codex): Edit only the `current:` line.
- **CLI** (`hivemind kpi bump <goal_id> <kpi_id> <delta>`, `src/commands/goal.ts:259`):
  reads content, regex-replaces the `current:` line by `+delta` (line 286), writes
  v=N+1. There is explicitly **no commit→KPI dedup** in v1 (double-bump on amend is
  user-corrected).

### 2.6 Two write channels (important nuance)

Because cursor/hermes/pi pre-tool-use hooks intercept **only Shell commands** (not
Write/Edit), the VFS classifier never fires there. So there are **two parallel
instruction variants** (`goals-instructions.ts`):
- `GOALS_INSTRUCTIONS` (VFS variant, line 29) — claude-code/codex use native
  Write/Edit on memory paths.
- `GOALS_INSTRUCTIONS_CLI` (line 47) — cursor/hermes/pi use `hivemind goal add/list/done/progress`
  and `hivemind kpi add/list/bump` shell commands. The CLI (`src/commands/goal.ts`)
  talks directly to the Deeplake API.

**Both land in the same tables.** This is the agent-agnostic guarantee.

### 2.7 Reading goals back (canonical reader)

`listOpenGoals` (`src/hooks/shared/context-renderer.ts:180`) is the shared reader:
- Owner match by **exact full form OR short form OR `short@%` alias** — never a
  `'%user%'` substring scan (which collides, e.g. `ali` matching `malice@…`).
- `status IN ('opened','in_progress')`.
- `version = MAX(version)` per `goal_id` (one row per goal).
- Defense-in-depth JS re-check of the owner forms (line 197).

Both the SessionStart context block and the open-goals banner share this reader so
they agree on counts (`open-goals.ts:60`). Goals render into the SessionStart
context as `=== HIVEMIND GOALS (N in_progress, M opened) ===` (line 243).

### 2.8 Commit-driven KPI auto-extract (currently disabled)

`src/hooks/commit-kpi-extract.ts` — a PostToolUse hook that intercepts successful
`git commit` (line 54), captures the diff (`git show HEAD`, capped 16kB, line 27),
and spawns the agent's native LLM (`claude -p` / `codex exec`) **detached** to read
active goals, judge whether the diff advanced any KPI, and bump it. **Disabled by
default** — `src/hooks/capture.ts:184` notes it "consumed a high amount of tokens"
(a full goal/KPI scan + reasoning pass per commit). Env `HIVEMIND_AUTO_KPI_FROM_COMMITS=false`.
This is the one place Hivemind tried "automatic success judging" and pulled back on cost.

---

## 3. Dashboard (`src/dashboard/`)

A **read-only HTML render** over local artifacts. Four files:

- **`data.ts`** — `loadDashboardData()` (line 270). Loads two streams, never writes
  back, never throws (every branch has an empty fallback):
  1. **KPI snapshot** (`loadKpis`, line 211): org-wide tokens saved via
     `fetchOrgStats` (same 1h cache as notifications) → local
     `~/.deeplake/usage-stats.jsonl` fallback → `tokensSource: "none"` empty-state.
     `DashboardKpis` (line 63): `tokensSaved`, `tokensSource` (`org`/`local`/`none`),
     `skillsCreated`, `memorySearches`, `sessionsCount`, `userTokensSaved`.
  2. **Codebase graph snapshot** — reads `~/.hivemind/graphs/<repo-key>/latest-commit.txt`,
     loads the JSON it points at (fallback: newest `*.json` under `snapshots/`).
     `null` when no snapshot → renderer shows "run `hivemind graph build`".
- **`render.ts`** — pure string-builder → self-contained HTML (line 1). Renders **KPI
  cards** (`renderKpiCards`, line 228: Tokens saved, Skills created, Memory recalls,
  Sessions) + an interactive **codebase graph** via one CDN script (vis-network 9.1.9,
  line 32). All external strings go through `escHtml` / `safeJsonForScript`.
- **`serve.ts`** — minimal `node:http` server for `hivemind dashboard --serve`
  (line 120). **Loopback-only** (`127.0.0.1:8123` default, line 53–54) so it's never
  LAN-exposed; EADDRINUSE → kernel-assigned ephemeral port fallback (line 132).
  Routes: `GET /` (the HTML), `GET /health` (204). Designed for Remote-SSH port-forwarding.
- **`open.ts`** — the CLI entry that loads data, renders, and either `xdg-open`s the
  file or starts the server on headless hosts.

So: **local web, single static page, KPI cards + graph, no auth surface, no live refresh.**
The dashboard is purely a *reporting* surface — it does not generate notifications.

---

## 4. Rules / success-judge / advisor proactive intelligence

Two distinct "rules" systems exist — don't confuse them:

1. **Notification rules** (`src/notifications/rules/`) — covered in §1.3. Pure
   functions that decide *what banner to fire*.

2. **Team principles / "advisor" rules** (`src/rules/` + `hivemind_rules` table) —
   org-wide *behavioral directives* injected into every agent's SessionStart context.
   - Schema `RULES_COLUMNS` (`deeplake-schema.ts:104`): `rule_id, text, scope
     (default 'team'), status (default 'active'), assigned_by, version, ...`.
   - Read via `listRules`/`getRuleLatest` (`src/rules/read.ts`), latest-version-per-id,
     default cap 10 (matches SessionStart inject cap).
   - Rendered as `=== HIVEMIND RULES (N active) ===` with the directive
     `"Treat any action that would violate one as a critical error and surface it
     to the user before proceeding"` (`context-renderer.ts:230,257`).
   - Managed via `hivemind rules` CLI (`src/commands/rules.ts`).

   **This is the closest thing to an "advisor":** standing principles the agent must
   check its actions against. It's a guardrail/conscience layer, not an LLM judge.

3. **Success-judge** — the only automatic outcome judge is the disabled
   commit-KPI-extract (§2.8): an LLM reads a git diff and judges KPI progress. The
   honest read: Hivemind *intentionally avoided* a heavy always-on success-judge for
   cost reasons and replaced it with **cheap deterministic signals** (regex pattern
   matching in cold-start, section parsing in resume brief, threshold gates).

---

## 5. Adoptable for Altevra

Altevra's roadmap (S4 Daily Briefing + proactive notifications) wants:
*"you decided X 3 months ago, still applies?"*, *"haven't talked to Srđan in 6 weeks"*,
*sleep/productivity pattern detection* — and crucially for a **personal-life** brain,
not just coding. Here's what Hivemind's architecture teaches, mapped to Altevra's
existing structure.

### 5.1 The notification framework shape is directly portable

Hivemind's `Notification` + `Rule` + source/delivery split is a clean, battle-tested
contract Altevra should mirror in its resident-agent layer:

- **Sources** (data → candidate notifications) ≈ Altevra's brain jobs / resident
  modes (insight, pattern detection, relationship-staleness scan).
- **Rules** (pure, IO-free `evaluate(ctx) → Notification | null`) ≈ Altevra's
  Relevance Gate. A rule like *"person not contacted in N weeks"* is exactly the
  `referral-invite` cadence-gate shape (`rules/referral-invite.ts:27`).
- **Delivery adapters** (per-agent) ≈ Altevra's universal-AI-tool integration. The
  `Record<Agent, EmitFn>` map (`delivery/index.ts:29`) is precisely the "adapter per
  tool, added one at a time" pattern Altevra already commits to.

**Steal the dedup model wholesale:** `state.shown[id] = {dedupKey, shownAt}` +
atomic `tryClaim` (`index.ts:119`). Altevra fires hooks from multiple tools that can
race — this O_EXCL claim file is the cheapest correct fix. The `dedupKey`-as-object
identity (`types.ts:26`) elegantly handles "re-fire when the underlying fact changes,
dedup when it's the same" — e.g. *"you decided X"* keyed on `{decision_id, version}`
re-fires only when the decision is revisited.

**Steal the `transient` / `userVisibleOnly` flags.** For a *personal* brain the
`userVisibleOnly` flag is even more load-bearing than for Hivemind: sensitive
personal insights (health, relationship patterns) should reach Pavle's eyes but must
NOT be injected into every downstream tool's model context. This maps 1:1 to
Altevra's sovereignty axiom and its trust ladder (sensitive memory = review-required,
local-models-only).

### 5.2 The proactive surfacing patterns map directly to Altevra's wishlist

| Altevra wish | Hivemind pattern to copy | Ref |
|---|---|---|
| "You decided X 3 months ago — still applies?" | Resume brief: walk newest-first over records, surface the most recent item with open/unresolved state, key on id+age | `resume-brief.ts:269,349` |
| "Haven't talked to Srđan in 6 weeks" | Cadence-gate rule (skip until N elapsed) + relative-age formatter | `referral-invite.ts:27`, `resume-brief.ts:248` (`relativeAge`) |
| "Pattern detected: late nights → worse code" | Cold-start `pickSignal` shape: cheap deterministic signals, **high-precision-or-silent**, one quantified insight + action | `cold-start-brief.ts:286,346` |
| Daily briefing / morning brief | `pickPrimaryBanner` = pick-ONE-priority-ranked + `composeBody` lead→brief→goals | `primary-banner.ts:108,208` |
| Active goals surfaced at session start | `listOpenGoals` + `=== GOALS ===` context block | `context-renderer.ts:180,243` |

**The single most valuable lesson: high-precision-or-silent.** Every Hivemind brief
returns `null` rather than surface a weak signal (`renderBrief` returns null on
`quiet`, `cold-start-brief.ts:347`; resume brief stays silent rather than say
"nothing pending", line 357). For a brain Pavle uses *daily for decades*, a noisy
proactive layer gets muted on week two. Altevra's Relevance Gate should be the rule
layer's pure `evaluate` returning null aggressively. This is the *same* discipline as
the CLAUDE.md "no Minecraft modpack research" directive — encoded as architecture.

### 5.3 The path-encodes-structure Goals/KPIs model — adopt selectively

The pattern: **structure in the path, prose in the body, no duplication, no drift.**
For Altevra this is genuinely elegant for any *living, status-tracked* entity:

```
goal/<owner>/<status>/<id>.md          → Hivemind
goal/<life-domain>/<status>/<id>.md    → Altevra (business/health/relationship/learning)
person/<name>/<last-contact-date>.md   → relationship staleness from the path
```

Why it's attractive for Altevra specifically:
- **Status = `mv`** is a beautiful UX for an agent: "move this goal to in_progress"
  is one filesystem op, atomic, audited (`goals-instructions.ts:40`).
- **Soft-close via `rm`** preserves the full audit trail (`goal-paths` + version-bump
  schema) — perfectly aligned with Altevra's "version history preserved / identity
  grows over decades" mandate.
- **Immutable version-bump rows** (`deeplake-schema.ts:127`) give Altevra free
  temporal provenance ("how did this goal/preference evolve?") — which the CLAUDE.md
  §4.5 identity-persistence requirement explicitly wants.

**But two cautions for Altevra:**
1. Altevra is **SQLite + one unified store** (CLAUDE.md §4.1), not a remote Deeplake
   VFS. You don't need the VFS *transport* — you need the *path-as-taxonomy* idea.
   Implement it as a `category_path` column / virtual hierarchy, not a real mount.
   Auto-categorization (CLAUDE.md §3.2) is exactly "assign the path"; the living
   taxonomy is "new path segments get proposed."
2. The **two-write-channel** complexity (VFS for some agents, CLI for others,
   `goals-instructions.ts`) is a tax Hivemind pays because it bolted onto 5 agents'
   differing hook capabilities. Altevra owns its MCP server + CLI — keep **one**
   write path (MCP/CLI to SQLite) and skip the VFS-classifier duplication entirely.

### 5.4 Dashboard — adopt the "static render over local artifacts" shape

For a personal brain, `serve.ts`'s **loopback-only, no-auth, single static page**
(`serve.ts:17,53`) is the right privacy posture (sovereignty axiom). The
`tokensSource: "org"|"local"|"none"` empty-state discipline (`data.ts:61`) — *always
render SOME page, distinguish "empty" from "zero"* — is a good habit for Altevra's
briefing UI on a fresh install.

### 5.5 What NOT to copy

- **No daemon / cron** — Hivemind's proactive layer fires *only* on session start.
  That's a limitation for Altevra: a personal brain genuinely needs time-based
  triggers ("it's 2:47am, you said late nights hurt your code"). Altevra should keep
  its brain-jobs/periodic-jobs layer (the architecture diagram already has it) and
  treat session-start as just *one* drain trigger — but reuse Hivemind's
  source/rule/delivery contract for *all* triggers (which the `Trigger` enum in
  `types.ts:13` was explicitly designed to allow: "the same Notification shape can be
  enqueued from any code path").
- **The disabled commit-KPI LLM judge** — Hivemind killed it for token cost
  (`capture.ts:184`). Altevra's multi-provider routing (cheap_worker / local_private)
  is the right answer here: run the success-judge on a cheap/local model, gated by the
  relevance layer, not on every event.

### 5.6 Concrete recommendation for Altevra S4

Build a `notifications` module mirroring Hivemind's three-layer split:
1. **Sources** = brain jobs emit candidate `Notification`s into a persistent queue
   (`enqueueNotification` equivalent) + pure pull-rules evaluated at trigger time.
2. **Rules = the Relevance Gate** — pure `evaluate(ctx) → Notification | null`,
   cadence-gated, high-precision-or-silent. This is where "personal vs business
   parity" lives: a relationship-staleness rule has equal standing to a deal-followup rule.
3. **Delivery** = per-tool adapters (Claude Code `additionalContext`/`systemMessage`,
   plus Obsidian daily-note append as a *human-canonical* channel — the equivalent of
   Hivemind's user-visible terminal channel). Use `userVisibleOnly` to keep sensitive
   personal insights out of model context by default.

Drains fire on: session start (Hivemind's only trigger), **plus** Altevra's periodic
brain jobs and a daily-briefing cron. Dedup with the `state.shown` + `dedupKey` model.

---

## Appendix — file index

Notifications:
- `src/notifications/index.ts` — `drainSessionStart` orchestrator
- `src/notifications/types.ts` — `Notification`/`Rule`/`Trigger`/state shapes
- `src/notifications/format.ts` — plain-text render
- `src/notifications/queue.ts`, `state.ts` — push queue + dedup state
- `src/notifications/rules/{registry,local-mined,referral-invite}.ts`
- `src/notifications/sources/{primary-banner,resume-brief,cold-start-brief,open-goals,org-stats,backend}.ts`
- `src/notifications/delivery/{index,claude-code}.ts`
- `src/notifications/AGENT_CHANNELS.md` — per-agent harness research

Goals/KPIs:
- `src/shell/goal-paths.ts` — path classifier + decompose/compose
- `src/shell/deeplake-fs.ts` — VFS routing + bootstrap-from-table
- `src/commands/goal.ts` — `hivemind goal`/`kpi` CLI
- `src/hooks/shared/goals-instructions.ts` — VFS + CLI agent instruction variants
- `src/hooks/shared/context-renderer.ts` — `listOpenGoals` + SessionStart block
- `src/hooks/commit-kpi-extract.ts` — disabled LLM success-judge
- `src/deeplake-schema.ts` — `GOALS_COLUMNS`/`KPIS_COLUMNS`/`RULES_COLUMNS`
- `src/config.ts` — table-name config

Dashboard:
- `src/dashboard/{data,render,serve,open}.ts`

Advisor/rules:
- `src/rules/{index,read,write}.ts` — team-principle rules
- `src/commands/rules.ts` — `hivemind rules` CLI

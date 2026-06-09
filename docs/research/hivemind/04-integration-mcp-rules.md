# Hivemind Integration Layer — Hooks, MCP, Rules, Install Flow

**Scope:** Deep documentation of how Hivemind (`@deeplake/hivemind` v0.7.84, Apache-2.0,
`/home/pavle/projekti/vendor/hivemind/`) wires into each AI-agent tool. Covers
`src/hooks/`, `src/mcp/`, `src/cli/install-*.ts`, `src/rules/`, and the top-level adapter
dirs (`claude-code/`, `codex/`, `openclaw/`, `hermes/`, `pi/`, `.claude-plugin/`).

**Reference frame:** Hivemind is a *team/org* shared-memory tool backed by a cloud SQL
store ("Deeplake"). Every session's events are captured to cloud tables; recall is exposed
either as a virtual filesystem at `~/.deeplake/memory/` (intercepted via PreToolUse hook
rewriting) or as MCP tools. This is architecturally adjacent to Altevra (local-first SQLite
second brain) but the *integration mechanics* are directly comparable and partly adoptable.

All file refs are `path:line` against the vendored tree.

---

## 0. The tier model (how each tool is wired)

Hivemind classifies tools into integration tiers. Source-of-truth comments live in
`src/cli/install-hermes.ts:8-30` and `src/cli/install-pi.ts:6-27`.

| Tool | Mechanism | Capture | Recall | Config touched |
|------|-----------|---------|--------|----------------|
| **Claude Code** | Marketplace plugin (`.claude-plugin/`) | hooks (6 events) | VFS via PreToolUse rewrite + skill | `claude` CLI plugin registry |
| **Codex** | Copied bundle + `~/.codex/hooks.json` | hooks (5 events) | VFS rewrite + `~/.agents/skills` symlink | `~/.codex/hooks.json`, `config.toml` feature flag |
| **Cursor** | Copied bundle + `~/.cursor/hooks.json` | hooks (6 events) | VFS rewrite | `~/.cursor/hooks.json` |
| **Hermes** | Bundle + `~/.hermes/config.yaml` | shell hooks (5 lifecycle) | **MCP tools** + VFS rewrite + skill | `~/.hermes/config.yaml` (`mcp_servers` + `hooks`) |
| **pi** | TS extension + `AGENTS.md` block | extension lifecycle events | **first-class pi tools** + VFS | `~/.pi/agent/AGENTS.md`, `extensions/` |
| **OpenClaw** | Copied plugin + allowlist patch | gateway capture | plugin tools + `hivemind context` | `~/.openclaw/openclaw.json` allowlist |
| **MCP (Tier B: Cline/Roo/Kilo)** | Shared MCP server at `~/.hivemind/mcp/server.js` | none (recall only) | MCP tools | per-consumer MCP config |

Two recall channels exist in parallel and are deliberately redundant (belt-and-suspenders,
see `install-hermes.ts:119-124`): (a) **MCP tools** `hivemind_search/read/index`, and (b) a
**PreToolUse Bash/grep interceptor** that rewrites any `grep`/`cat` against
`~/.deeplake/memory/` into a single backend SQL query. If the agent ignores skill guidance
and shells a raw grep, accuracy still matches the MCP path.

---

## 1. How Hivemind wires into each tool

### 1.1 Claude Code — marketplace plugin (no config-file editing)

Claude Code is the *only* tool Hivemind does **not** wire by editing a hooks file. Instead it
delegates entirely to Claude Code's plugin loader (`src/cli/install-claude.ts:7-19`):

```
claude plugin marketplace add activeloopai/hivemind
claude plugin install hivemind
claude plugin enable hivemind@hivemind
```

- Marketplace descriptor: `.claude-plugin/marketplace.json` — points at the GitHub repo,
  `source: git-subdir`, `path: claude-code`, pinned `sha`.
- Plugin manifest: `.claude-plugin/plugin.json` and `claude-code/.claude-plugin/plugin.json`
  (identical — name `hivemind`, Apache-2.0).
- Hook wiring ships **inside the plugin** at `claude-code/hooks/hooks.json`. Claude Code's
  loader auto-registers these; Hivemind never writes to `~/.claude/settings.json`.
  - `install-claude.ts:78-97` documents a past regression (v0.7.23/24) where an earlier
    helper *did* write literal paths into `settings.json` and broke hooks. The current code
    actively **removes** those broken entries on every install (`cleanupBrokenSettingsHooks`,
    `install-claude.ts:160-217`).
- Commands: `claude-code/commands/{login.md,update.md}` (slash commands).
- Skills: `claude-code/skills/{hivemind-goals,hivemind-graph,hivemind-memory}/`.

**Events hooked** (`claude-code/hooks/hooks.json`). `${CLAUDE_PLUGIN_ROOT}` is resolved by the
loader at runtime:

| Event | Bundle script(s) | Timeout | Async | Purpose |
|-------|------------------|---------|-------|---------|
| `SessionStart` | `session-start.js` | 10s | no | inject memory instructions + RULES/GOALS block, create placeholder summary row |
| | `session-notifications.js` | 8s | no | user-visible banners (resume brief, balance, mined skills) |
| | `session-start-setup.js` | 120s | **yes** | heavy DB ensure-table / version check off the critical path |
| `UserPromptSubmit` | `capture.js` | 10s | yes | capture the user prompt as a session row |
| `PreToolUse` | `pre-tool-use.js` | 60s | no | intercept + rewrite memory-VFS commands (must be sync to mutate tool_input) |
| `PostToolUse` | `capture.js` | 15s | yes | capture tool call row |
| `Stop` | `capture.js` + `graph-on-stop.js` | 30s | yes | capture assistant turn; auto-build code graph |
| `SubagentStop` | `capture.js` | 30s | yes | capture subagent turn |
| `SessionEnd` | `session-end.js` + `plugin-cache-gc.js` + `graph-on-stop.js` | 60/15/30s | mixed | spawn wiki-worker summary; GC; graph build |

### 1.2 Codex — copied bundle + `~/.codex/hooks.json` merge

`src/cli/install-codex.ts:230-276`:
1. Copy `codex/bundle` → `~/.codex/hivemind/bundle/`, `codex/skills` → `~/.codex/hivemind/skills/`.
2. `codex features enable hooks` and strip the legacy `codex_hooks` feature key from
   `config.toml` (renamed in codex 0.130.0) — `install-codex.ts:193-228`.
3. Merge our hook entries into `~/.codex/hooks.json` (see §4 merge logic).
4. Symlink the skill into the shared agentskills.io location
   `~/.agents/skills/hivemind-memory` (`install-codex.ts:252-258`).
5. Symlink `node_modules` → shared `~/.hivemind/embed-deps/node_modules` so the tree-sitter
   native module resolves for `graph-on-stop.js`.

Events (`codex/hooks/hooks.json`, also rebuilt in `install-codex.ts:25-39`): `SessionStart`
(matcher `startup|resume`), `UserPromptSubmit`→capture, `PreToolUse` (matcher `Bash`)→rewrite,
`PostToolUse`→capture, `Stop`→`stop.js` + `graph-on-stop.js`. **No** `SessionEnd` — Codex
uses `Stop`'s `stop.js` for summary spawning.

### 1.3 Cursor — copied bundle + `~/.cursor/hooks.json` (v1 schema)

`src/cli/install-cursor.ts:101-125`. Cursor 1.7+ hooks API. **Schema differs** from
Claude/Codex (`install-cursor.ts:6-13`): top-level `{ "version": 1, "hooks": {...} }`, array
entries are command objects **directly** (no `{matcher, hooks:[...]}` wrapper), camelCase event
names. Events (`install-cursor.ts:44-60`): `sessionStart`, `beforeSubmitPrompt`→capture,
`preToolUse` (matcher `Shell`)→rewrite, `postToolUse`→capture, `afterAgentResponse`→capture,
`stop`→capture+graph, `sessionEnd`→session-end+graph.

### 1.4 Hermes — `~/.hermes/config.yaml` (3 surfaces: skill + MCP + hooks)

`src/cli/install-hermes.ts:178-218`. Hermes is the richest integration — it gets **all three**
surfaces:
1. **Skill** at `~/.hermes/skills/hivemind-memory/SKILL.md` (body inlined `install-hermes.ts:39-86`).
2. **MCP server** registered in `config.yaml` → `mcp_servers.hivemind = {command:"node", args:[MCP_SERVER_PATH]}` (`install-hermes.ts:201-211`).
3. **Shell hooks** in `config.yaml` → `hooks:` (`buildHooksBlock`, `install-hermes.ts:116-133`).
   Hermes lifecycle events (Claude-Code-shaped JSON on stdin):
   - `on_session_start` → `session-start.js`
   - `pre_tool_call` (matcher `terminal`) → `pre-tool-use.js` (grep interceptor)
   - `pre_llm_call` / `post_tool_call` / `post_llm_call` → `capture.js`
   - `on_session_end` → `session-end.js` + `graph-on-stop.js`
4. Also sets `hooks_auto_accept: true` so hooks fire without a per-use consent prompt
   (`install-hermes.ts:213-216`).

### 1.5 pi — TS extension + `AGENTS.md` block (no MCP)

`src/cli/install-pi.ts:107-180`. pi (badlogic/pi-mono) has no MCP and no SessionStart hook in
the Claude sense; instead:
1. Upsert a marker-delimited `<!-- BEGIN hivemind-memory -->...<!-- END -->` block into
   `~/.pi/agent/AGENTS.md` (pi auto-loads this every turn) — `install-pi.ts:77-105`.
2. Copy a raw TS extension `~/.pi/agent/extensions/hivemind.ts` (pi compiles it on load). It
   subscribes to pi's 25+ lifecycle events (`session_start`, `input`, `tool_call`,
   `tool_result`, `message_end`, `session_shutdown`) and registers `hivemind_search/read/index`
   as **first-class pi tools** (`install-pi.ts:12-27`).
3. Copy four standalone worker bundles (wiki, skillify, autopull, skillopt) that the extension
   shells out to on the relevant events (`install-pi.ts:130-161`).

pi gets rules/goals via the `hivemind context` CLI on demand, not via a hook
(`src/commands/context.ts:8-15`).

### 1.6 OpenClaw — copied plugin + allowlist patch

`src/cli/install-openclaw.ts:9-80`. Copies `openclaw/dist` → `~/.openclaw/extensions/hivemind/`,
then **patches `~/.openclaw/openclaw.json`** so `plugins.allow` actually loads the plugin
(`ensureHivemindAllowlisted`, `install-openclaw.ts:56`). Safe-by-default: only touches explicit
allowlists; skips silently if config absent/malformed. Manifest `openclaw/openclaw.plugin.json`
declares contract `tools: [hivemind_search, hivemind_read, hivemind_index, hivemind_goal_add,
hivemind_kpi_add]` + commands + config toggles (autoCapture/autoRecall/autoUpdate).

---

## 2. The MCP server (`src/mcp/server.ts`)

A stdio MCP server (`@modelcontextprotocol/sdk`) spawned as a subprocess by the consuming
client (Hermes today; reused by any MCP-aware agent — see header `server.ts:8-13`). It is the
*recall-only* surface — **no write/capture tools**.

**Auth:** loads `~/.deeplake/credentials.json`; if missing, every tool returns a clean
"Not authenticated. Run `hivemind login`" string rather than crashing (`server.ts:32-43`).

**Tools exposed** (registered via `server.registerTool`):

| Tool | Args | Backend query | Source |
|------|------|---------------|--------|
| `hivemind_search` | `query: string`, `limit?: 1-50` | hybrid lexical search across memory + sessions tables via `searchDeeplakeTables` (fixed-string, case-insensitive) | `server.ts:54-93` |
| `hivemind_read` | `path: string` (must start `/`) | `SELECT summary/message FROM <table> WHERE path=...` — routes `/sessions/` to sessions table, else memory table | `server.ts:95-126` |
| `hivemind_index` | `prefix?: string`, `limit?: 1-200` | `SELECT path,description,project,last_update_date ... ORDER BY last_update_date DESC` | `server.ts:128-166` |

Security notes worth stealing: descriptions tell the model *"different paths under
`/summaries/<username>/` are different users — do not merge them"* (`server.ts:57`); the index
prefix is escaped with `sqlLike` **including LIKE wildcards** so an LLM-supplied `prefix='%'`
can't dump every row (`server.ts:141-148`).

Transport: `StdioServerTransport`, `main()` at `server.ts:168-176`. The shared server binary
is installed to `~/.hivemind/mcp/server.js` and registered by absolute path; descriptor builder
at `src/cli/install-mcp-shared.ts:31-37` (`{command:"node", args:[MCP_SERVER_PATH]}`).

---

## 3. Rules — cross-agent "team principles" injected at SessionStart

This is the most directly adoptable subsystem for Altevra. (`src/rules/`, rendered by
`src/hooks/shared/context-renderer.ts`.)

### 3.1 Storage model — append-only, versioned

`src/rules/write.ts`. The `hivemind_rules` table is **INSERT-only**: every edit appends a new
row with `version+1`; no UPDATEs (the Deeplake backend silently coalesces rapid UPDATEs — see
`write.ts:1-11`). Reads (`src/rules/read.ts:52-84`) fetch all rows and keep the latest version
per `rule_id` (JS dedup, deterministic three-key tiebreak `version DESC, created_at DESC,
id DESC`). Rules are org-wide (`scope: 'team'`, hardcoded `write.ts:84`). Helpers: `insertRule`,
`editRule`, `markRuleDone` (`write.ts:84-151`), barrel-exported via `src/rules/index.ts`.

### 3.2 Injection at SessionStart

`src/hooks/shared/context-renderer.ts:77-139` — one shared `renderContextBlock(query, input, opts)`
that every agent's SessionStart fork imports and concatenates onto its DEEPLAKE-MEMORY context.
It queries two tables:
- `hivemind_rules` (active rules, `listRules`)
- `hivemind_goals` (current user's open goals, `listOpenGoals` — `context-renderer.ts:173-215`,
  filtered by canonical owner forms to avoid substring collisions, capped at 10 each).

Output format (`formatBlock`, `context-renderer.ts:224-265`):
```
=== HIVEMIND RULES (N active) ===
- <rule_id>: <text>
=== HIVEMIND GOALS (X in_progress, Y opened) ===
[in_progress] <goal_id>: <first line>
=== HIVEMIND HOW-TO ===
- Rules above are team principles. Treat any action that would violate one as a
  critical error and surface it to the user before proceeding.
```

Wired into Claude SessionStart at `src/hooks/session-start.ts:227-269` (only when
authenticated; absorbs all errors → `""`). Codex deliberately **omits** the rules block —
Codex's `additionalContext` is user-visible (`hook context:` cell) and would clobber the TUI
(`src/hooks/codex/session-start.ts:116-126`); Codex/pi/openclaw fetch via `hivemind context`
CLI instead (`src/commands/context.ts:8-15`, `src/cli/index.ts:136-137`).

### 3.3 Enforcement model

**There is no hard enforcement.** Rules are injected as **prompt text** and the model is
*instructed* to treat violations as critical and surface them
(`context-renderer.ts:257`). This is advisory/soft. The only hard guarantee is
**prompt-injection defense**, applied at two layers:
- **Write-time:** `assertValidText` rejects newlines (CR/LF/U+2028/U+2029/U+0085) and >2000
  chars so a rule can't inject a fake `=== HIVEMIND HOW-TO ===` section (`write.ts:65-77`).
- **Render-time:** `sanitizeForInject` replaces any line terminator with literal `\n` for
  rows persisted by older clients or coming from the VFS write path
  (`context-renderer.ts:281-308`).

CLI surface: `hivemind rules add|list|edit|done` (`src/cli/index.ts:130-137`,
`src/commands/rules.ts`).

---

## 4. The install flow (`hivemind install`)

Entry: `src/cli/index.ts:339-371`. `hivemind install` auto-detects assistants (`detectPlatforms()`)
or honors `--only <csv>`, runs an auth gate, then `runSingleInstall(id)` per tool
(`index.ts:373-384`). Per-tool dispatch is a switch over `claude|codex|claw|cursor|hermes|pi`.

### 4.1 Scopes

- Claude Code is multi-scope: `user | project | local | managed`. Updates fan out across all
  four (`install-claude.ts:76`, `:252-254`) — scopes the user hasn't activated simply error
  harmlessly.
- All other tools install at **user-global** scope (`~/.codex/`, `~/.cursor/`, `~/.hermes/`,
  `~/.pi/`). There is no project-local install path in Hivemind.

### 4.2 Auth gate (install ≠ auth)

`runAuthGate` (`index.ts:206-337`): token via `--token`/`$HIVEMIND_TOKEN` (CI), else TTY consent
prompt + device-flow browser login, else API-key paste fallback (3 attempts). Every failure path
**continues the install** — hooks land, user can `hivemind login` later. Notably it can offer to
scan recent Claude sessions for repeatable mistakes before the sign-in pitch.

### 4.3 How it avoids clobbering existing config

This is the strongest part and the most relevant to Altevra. Each tool has an
identify-strip-append merge:

- **Codex** (`install-codex.ts:120-150`): `mergeHooks(existing, ours)` strips prior Hivemind
  entries (recognized by `isHivemindHookEntry` — command path contains `<pluginDir>/bundle/` OR
  ends in a known Hivemind bundle filename, `install-codex.ts:80-98`), drops now-empty events,
  appends ours, **preserves all other events and top-level keys**. Windows backslash paths are
  normalized before matching (a real "PostToolUse runs twice" bug, `:88-92`). Foreign (dev-clone)
  Hivemind hooks are reported before stripping (`:170-191`).
- **Cursor** (`install-cursor.ts:62-99`): same strip-by-path-match (`/.cursor/hivemind/bundle/`)
  + a top-level `_hivemindManaged` marker.
- **Hermes** (`install-hermes.ts:135-154`): `mergeHooks` per-event strip+append; identifies via
  `cmd.includes("/.hermes/hivemind/bundle/")`. Uninstall also removes `hooks_auto_accept` it
  added (`:249-252`).
- **pi** (`install-pi.ts:77-105`): marker-block upsert in `AGENTS.md` — strips the old
  `BEGIN/END hivemind-memory` block and re-appends, preserving surrounding content.
- **OpenClaw**: only patches explicit allowlists, never flips default-allow into restrictive
  mode (`install-openclaw.ts:51-55`).

**Idempotency:** `writeJsonIfChanged` skips byte-identical rewrites so the tool's hooks-file
trust fingerprint isn't perturbed and the user isn't re-prompted to re-trust hooks
(`install-codex.ts:246-250`, `install-cursor.ts:111-114`). There is **no backup-before-write**
step in any Hivemind installer — it relies on the merge being non-destructive.

**Uninstall** is symmetric: strip only Hivemind entries, delete the file only if nothing
meaningful remains, keep plugin files on disk for a cheap reinstall (`install-codex.ts:278-314`,
`install-cursor.ts:127-147`, `install-hermes.ts:220-262`).

---

## 5. Mapping onto Altevra's existing adapter model

Altevra's equivalent is `altevra install-hooks`
(`crates/altevra-cli/src/commands/install_hooks.rs`) plus the `altevra-adapters` crate
(`claude_code.rs`, `codex.rs`, `cursor.rs`, `cursor_cli.rs`, `antigravity.rs`, `hermes.rs`,
`hermes_ingest_sh.rs`, `factory.rs`). Capture flows through one binary entrypoint
`altevra hook-handle <event> --tool <tool>` (`crates/altevra-cli/src/commands/hook_handle.rs`),
which parses the tool's hook JSON from stdin into a turn row. Recall is via the
`altevra-mcp` server (40+ tools incl. `get_agent_bootstrap_packet`, `get_context_packet`,
`build_system_prompt`, `search_memory`, `search_wiki`, `save_decision`, `get_goals`…).

| Concern | Hivemind | Altevra |
|---------|----------|---------|
| **Claude Code wiring** | marketplace plugin, never edits settings.json | edits `~/.claude/settings.json` directly (`patch_claude_code`, `install_hooks.rs:312-315`) |
| **Codex/Cursor/Hermes** | copied JS bundle + per-tool merge | single `altevra hook-handle` command string, no bundle copy (`install_hooks.rs:421-431`) |
| **Capture transport** | per-tool Node bundle scripts | one Rust binary, event passed as CLI arg |
| **Recall** | MCP (3 tools) + VFS grep-rewrite | MCP (40+ tools), no VFS |
| **Event matrix** | 5-6 events/tool | 5 events Claude/Codex, 3 Cursor, 5 Hermes (`install_hooks.rs:276-306`) |
| **Hermes bridge** | JS bundle + config.yaml | `altevra-ingest.sh` script copied to `~/.hermes/hooks/`, chmod 0755 (`install_hooks.rs:638-653`) |
| **Idempotency** | `_hivemindManaged` marker + writeJsonIfChanged | `_altevra_managed` marker + drift detection (`install_hooks.rs:679-719`) |
| **Backup** | none (relies on non-destructive merge) | **unconditional `cp` backup before every write** (`install_hooks.rs:245-262`) |
| **Self-skip** | n/a | SI-6 two-layer gate: refuses to wire when cwd or target is inside the Altevra repo (`install_hooks.rs:125-149`, `:753-843`) |
| **Project scoping** | user-global only | smart auto-scope: scores repos, writes project-local `.claude/settings.json` for score≥3 (`install_hooks.rs:892-957`) |
| **Rules/team principles** | `hivemind_rules` injected at SessionStart | **no equivalent** — Altevra injects identity/goals/context-packet via MCP `get_agent_bootstrap_packet` / `build_system_prompt`, not a SessionStart-hook text block |

**Schema convergence:** Altevra already understands the *exact same* config schemas Hivemind
targets (`install_hooks.rs:43-53` documents claude-code/codex/cursor/hermes formats), and uses
the same camelCase-no-matcher Cursor v1 shape and snake_case Hermes YAML shape. So the two tools
are wiring the *same files with the same schemas* — see §6 coexistence.

**Key gaps Altevra has that Hivemind solves better:**
- Altevra has no equivalent of the **SessionStart rules/goals text-injection block** as a
  *hook-time* surface (it relies on the agent proactively calling MCP). Hivemind pushes a
  compact RULES/GOALS block into context unconditionally at session start.
- Altevra's per-tool patch logic is good but **does not copy a capture bundle** — it relies on
  the `altevra` binary being on PATH. Hivemind's bundle-copy + PATH-independent
  `${CLAUDE_PLUGIN_ROOT}` resolution is more robust to PATH issues (cf. Altevra's documented
  "altevra: command not found" PATH problem in CLAUDE.md §7).

---

## 6. Adoptable for Altevra

### 6.1 What's better in Hivemind's integration — worth adopting

1. **SessionStart RULES/GOALS injection block (`context-renderer.ts`).** The single biggest
   adoptable idea. A *shared, agent-agnostic renderer* that:
   - reads a small set of rows (rules + open goals) and formats a compact, capped block,
   - is injected at SessionStart for hook-capable tools and exposed as a CLI
     (`hivemind context`) for hook-less tools (pi/openclaw),
   - degrades to `""` on any error so it never blocks session start,
   - has **two-layer prompt-injection defense** (reject newlines at write, sanitize at render).

   Altevra's `build_system_prompt` / `get_agent_bootstrap_packet` MCP tools already assemble
   richer context, but they fire only when the agent *chooses* to call them. Adopting a
   **SessionStart-hook text block** (rendered once by a shared function, injected as
   `additionalContext` for Claude/Cursor/Hermes and printed by `altevra context` for the rest)
   would give Altevra unconditional context delivery — which matches the CLAUDE.md vision §3.5
   ("bootstrap context loaded at session start"). This is a near-direct port; Altevra's
   `hook_handle session_start` could emit the block instead of just opening a session.

2. **The per-tool merge abstraction (identify → strip → append).** Hivemind's
   `isHivemindHookEntry` / `mergeHooks` pattern with **path-separator normalization** and
   **foreign-clone detection** is more battle-tested than Altevra's marker-only approach for the
   case where the *command path* (not a JSON marker) is the only stable identifier. Altevra's
   marker-based dedup is actually cleaner where a marker survives round-trips, but Hivemind's
   "recognize by known bundle-filename set" fallback is useful insurance against marker loss.

3. **Codex-specific channel awareness.** Hivemind learned (the hard way, documented at
   `codex/session-start.ts:94-126`) that Codex renders `additionalContext` **user-visibly**, so
   it deliberately ships a *minimal* SessionStart payload to Codex and routes rules/goals to a
   pull-based CLI. Altevra should encode the same per-tool channel matrix
   (`src/notifications/AGENT_CHANNELS.md` is Hivemind's source of truth for this) before adopting
   idea #1 — blindly injecting a big block into Codex would clobber its TUI.

4. **`writeJsonIfChanged` trust-fingerprint preservation.** Codex/Cursor re-prompt for hook
   trust on any byte change. Altevra currently writes whenever `added_events` is non-empty
   (`install_hooks.rs:402-406`) which is already good, but Hivemind's stricter "skip even
   byte-identical rewrites" is a slightly cleaner guarantee for re-runs.

5. **MCP prompt-injection hardening in tool descriptions.** The `sqlLike`-escapes-wildcards
   trick (`server.ts:141-148`) and the "different users — do not merge" instruction are cheap,
   high-value patterns for Altevra's MCP tools.

### 6.2 What Altevra does better (keep)

- **Backup-before-write** (`install_hooks.rs:245-262`) — Hivemind has none. Keep.
- **SI-6 self-skip** — prevents the feedback loop of Altevra capturing its own dev sessions.
  Hivemind has no concept of this (it's a team tool, not its own subject). Keep.
- **Smart project scoping** — Hivemind is user-global only. Keep Altevra's per-repo scoring.
- **Single-binary capture** — no per-tool JS bundle to keep in sync; simpler to ship. Keep,
  but fix the PATH issue (Hivemind's bundle-copy approach is the alternative if PATH proves
  fragile).

### 6.3 Can Altevra + Hivemind coexist on the same machine?

**Yes, with one real caveat.** They target the **same config files with the same schemas**:

| File | Hivemind writes | Altevra writes | Conflict? |
|------|-----------------|----------------|-----------|
| `~/.claude/settings.json` | **no** (marketplace plugin → its own hooks.json) | yes (managed entries w/ `_altevra_managed`) | **No** — disjoint surfaces |
| `~/.codex/hooks.json` | yes (`<pluginDir>/bundle/*` + `_hivemind`-by-path) | yes (`_altevra_managed` marker) | **No** — each strips only *its own* entries; both append additively |
| `~/.cursor/hooks.json` | yes (`/.cursor/hivemind/bundle/` + `_hivemindManaged`) | yes (`_altevra_managed`) | **No** — disjoint identification keys; both additive |
| `~/.hermes/config.yaml` | yes (`mcp_servers.hivemind` + hooks `/.hermes/hivemind/bundle/`) | yes (`altevra-ingest.sh` + `altevra_managed`) | **No** — disjoint keys; both merge additively. Both set their own MCP server key (`hivemind` vs none/`altevra`) |
| `~/.agents/skills/` | symlink `hivemind-memory` | Altevra skills (managed) | **No** — different skill names |

Why coexistence works: **both tools use additive, marker/path-scoped merges that strip only
their own entries.** Hivemind's `isHivemindHookEntry` matches on `/bundle/` paths and Hivemind
bundle filenames; Altevra matches on the `_altevra_managed` boolean. Neither recognizes the
other's entries, so neither strips the other's, and both append to the same event arrays.

**The one real caveat — double-capture / overhead, not corruption:** on Codex/Cursor/Hermes,
**both tools' hooks fire on every event**, so every prompt/tool-call/turn is captured twice
(once into Deeplake cloud, once into Altevra SQLite) and two PreToolUse interceptors run on
Bash commands. Functional risk is low (Hivemind's PreToolUse only *rewrites* commands that
touch `~/.deeplake/memory/`; Altevra's `pre_tool_use` only records), but:
- there's added latency per event (two Node/binary spawns),
- two SessionStart blocks may both inject context (Hivemind RULES/GOALS + an Altevra block if
  idea #1 is adopted) — manageable but worth coordinating,
- on Claude Code there's **zero conflict** because Hivemind never touches `settings.json`.

**Recommendation:** coexistence is safe today. If Altevra adopts SessionStart injection
(§6.1#1), gate it per-tool using the same channel matrix Hivemind uses (`AGENT_CHANNELS.md`)
and consider a `--skip-tool` flag so a user running both can let Hivemind own Codex/Cursor capture
while Altevra owns Claude Code — avoiding double-capture without losing either tool's strengths.

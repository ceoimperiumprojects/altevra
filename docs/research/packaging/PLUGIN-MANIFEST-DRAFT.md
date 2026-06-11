# Altevra Claude Code Plugin — Manifest Draft (prep only, NOT publishing)

**Date:** 2026-06-11
**Status:** Research + concrete draft. No code touched, nothing published.
**Sources verified on this machine (current, not training data):**
- `~/.claude/plugins/marketplaces/claude-plugins-official/` — local clone of
  `anthropics/claude-plugins-official` (canonical marketplace.json + 37 first-party plugins)
- `.../plugins/plugin-dev/skills/{plugin-structure,mcp-integration,hook-development}/` —
  Anthropic's own authoritative plugin format docs (incl. `references/manifest-reference.md`)
- `/home/pavle/projekti/vendor/hivemind/claude-code/` — known-working third-party plugin
  (skills + hooks + commands, published via own `marketplace.json`)
- `~/.claude/plugins/marketplaces/imperium-startup-marketplace/` — Pavle's own already-published
  marketplace (precedent: `source: "./"` single-plugin repo)
- `claude plugin --help` / `claude plugin marketplace --help` (live CLI, v2026-06)
- `~/.claude/plugins/{known_marketplaces.json,installed_plugins.json}` — observed install flow state

---

## 1. The EXACT current format (verified)

### 1.1 Required files

A **plugin** needs exactly one required file:

```
plugin-root/
└── .claude-plugin/
    └── plugin.json          # REQUIRED — only `name` is mandatory
```

Everything else is optional and auto-discovered **at plugin root** (NOT inside `.claude-plugin/`):

```
plugin-root/
├── .claude-plugin/plugin.json   # required manifest
├── commands/*.md                # slash commands (md + YAML frontmatter)
├── agents/*.md                  # subagents (md + YAML frontmatter)
├── skills/<skill-name>/SKILL.md # skills — one DIRECTORY per skill, file MUST be SKILL.md
├── hooks/hooks.json             # hook config (or inline in plugin.json `hooks`)
├── .mcp.json                    # MCP servers (or inline in plugin.json `mcpServers`)
└── scripts/                     # helpers
```

A **marketplace** is any git repo / path with:

```
repo-root/
└── .claude-plugin/
    └── marketplace.json     # REQUIRED for a marketplace
```

A single repo can be both (hivemind does this: root `marketplace.json` pointing at
`claude-code/` subdir which holds the actual plugin with its own `.claude-plugin/plugin.json`).
Pavle's imperium-startup-marketplace uses the simplest variant: `"source": "./"`
(the marketplace repo root IS the plugin).

### 1.2 plugin.json schema (full field set)

```jsonc
{
  "name": "altevra",                  // REQUIRED. kebab-case, /^[a-z][a-z0-9]*(-[a-z0-9]+)*$/
  "version": "0.5.0",                 // semver MAJOR.MINOR.PATCH (default "0.1.0" if omitted)
  "description": "…",                 // 50–200 chars recommended (marketplace display)
  "author": { "name": "…", "email": "…", "url": "…" },  // or string "Name <email> (url)"
  "homepage": "https://…",
  "repository": "https://github.com/…",   // or {type, url, directory}
  "license": "SPDX-id",
  "keywords": ["…"],
  // Optional component-path overrides — SUPPLEMENT defaults, never replace.
  // Must be relative, must start with "./", no "../", forward slashes only:
  "commands": "./commands" | ["./a", "./b"],
  "agents":   "./agents"   | [...],
  "hooks":    "./hooks/hooks.json" | { inline hook object },
  "mcpServers": "./.mcp.json"      | { inline server object }
}
```

Validation notes (from manifest-reference.md): name conflicts across installed plugins
error out; custom paths are merged with defaults; component name conflicts are errors.

### 1.3 Skills declaration

No registry/list anywhere — **pure auto-discovery**: every `skills/<dir>/SKILL.md` loads.
SKILL.md frontmatter:

```markdown
---
name: skill-name
description: When to use this skill (this is the trigger text Claude matches on)
version: 1.0.0
---
Body = instructions. Supporting files allowed in the skill dir (scripts/, references/, examples/).
```

> ⚠️ Altevra's `06-skills/*.md` use `slug:`/`title:`/`tools:` frontmatter — a **conversion step**
> (slug→name, description kept, flat file → `skills/<name>/SKILL.md`) is required for the plugin.

### 1.4 MCP servers declaration

`.mcp.json` at plugin root (recommended) or inline `mcpServers` in plugin.json.
Server entry shapes (verified in example-plugin + mcp-integration skill):

```jsonc
// stdio (what Altevra needs):
{ "altevra": { "command": "altevra", "args": ["serve", "--vault", "…"], "env": {"K":"${V}"} } }
// remote:
{ "x": { "type": "sse",  "url": "https://…" } }
{ "x": { "type": "http", "url": "https://…", "headers": { "Authorization": "Bearer ${TOK}" } } }
```

- `${CLAUDE_PLUGIN_ROOT}` expands to the installed plugin dir — use for any bundled file.
- `${ENV_VAR}` expansion works in `env`/`headers`/`args`.
- Servers auto-start when the plugin is enabled; stdio process lifecycle is managed by Claude Code.

### 1.5 Hooks declaration

`hooks/hooks.json` (or inline). Format — same event structure as `settings.json` hooks,
wrapped with an optional top-level `description` (verified in hivemind):

```json
{
  "description": "what these hooks do",
  "hooks": {
    "SessionStart": [ { "matcher": "*", "hooks": [
      { "type": "command", "command": "altevra hook-handle session_start --tool claude-code",
        "timeout": 5, "async": true }
    ] } ]
  }
}
```

Events: `PreToolUse, PostToolUse, UserPromptSubmit, Stop, SubagentStop, SessionStart,
SessionEnd, PreCompact, Notification`. Fields per hook: `type:"command"`, `command`,
`timeout` (seconds), `async` (hivemind uses it heavily to keep capture non-blocking).
Hooks register on plugin enable — **this replaces Altevra's manual `install-hooks` wiring
of `~/.claude/settings.json` for Claude Code users.**

### 1.6 marketplace.json schema

```jsonc
{
  "$schema": "https://anthropic.com/claude-code/marketplace.schema.json",  // optional
  "name": "marketplace-name",                 // kebab-case
  "description": "…",
  "owner": { "name": "…", "email": "…" },
  "metadata": { "description": "…", "version": "…" },   // optional (hivemind uses it)
  "plugins": [
    {
      "name": "plugin-name",
      "description": "…",
      "version": "0.5.0",
      "author": { "name": "…" },
      "category": "productivity",            // free-ish taxonomy: development/security/design/productivity…
      "homepage": "https://…",
      "source": <SOURCE>                      // see below
    }
  ]
}
```

**Source variants observed in the official marketplace:**

| Variant | Shape | Use |
|---|---|---|
| relative path | `"source": "./plugins/foo"` or `"./"` | plugin lives inside the marketplace repo |
| git url | `{ "source": "url", "url": "https://…/repo.git", "sha": "<commit>" }` | whole external repo is the plugin |
| git subdir | `{ "source": "git-subdir", "url": "…", "path": "plugins/foo", "ref": "v1.5.5", "sha": "…" }` | plugin in a subdir of external repo, pinnable to ref+sha |
| github | `{ "source": "github", "repo": "owner/repo" }` | seen in `known_marketplaces.json` for marketplace sources |

`sha` pinning is what the official marketplace uses for supply-chain integrity.

### 1.7 Install flow + versioning (live CLI, verified)

```bash
# user adds the marketplace (GitHub repo, URL, or local path):
claude plugin marketplace add ceoimperiumprojects/altevra      # or a path / git URL
# then installs:
claude plugin install altevra@altevra        # plugin@marketplace
claude plugin enable|disable|update|uninstall altevra
claude plugin marketplace update [name]      # refresh marketplace clone
# dev loop without a marketplace:
claude plugin init <name>                    # scaffolds at ~/.claude/skills/<name>/ (auto-loads as <name>@skills-dir)
# release tagging:
claude plugin tag [path]                     # creates {name}--v{version} git tag, VALIDATES that
                                             # plugin.json and the enclosing marketplace entry agree on version
```

Install state lands in `~/.claude/plugins/`: marketplace clone under `marketplaces/<name>/`,
plugin copy under `cache/<marketplace>/<plugin>/<version>/`, registry in
`installed_plugins.json` (records scope, version, gitCommitSha), marketplace sources in
`known_marketplaces.json`. Versioning is plain semver in plugin.json; `claude plugin update`
re-pulls from source; `claude plugin tag` is the release-discipline tool.

**`npx skills`** — separate ecosystem (skills.sh CLI for distributing bare agent skills across
Claude Code / Codex / Cursor etc., installs into `.claude/skills/` or agent-equivalent dirs).
Not required for marketplace shipping; optional secondary channel later for the two
agent-facing skills only. The Claude-native path above is the primary target.

---

## 2. Proposed `.claude-plugin/` package for Altevra (DRAFT)

Recommended repo layout: a `plugin/` (or `claude-plugin/`) subdir in the altevra repo,
referenced from a root marketplace.json — keeps crates/ untouched and lets the same repo
serve as its own marketplace (hivemind pattern):

```
altevra/
├── .claude-plugin/
│   └── marketplace.json            # repo doubles as marketplace
└── plugin/                         # the plugin root
    ├── .claude-plugin/
    │   └── plugin.json
    ├── skills/
    │   ├── altevra-core/SKILL.md            # from 06-skills/altevra-core.md (frontmatter converted)
    │   └── altevra-agent-operations/SKILL.md# from 06-skills/altevra-agent-operations.md
    ├── hooks/
    │   └── hooks.json
    ├── .mcp.json
    ├── commands/
    │   └── altevra-setup.md        # optional: guided first-run (binary check + `altevra init`)
    └── README.md                   # install prerequisites (Rust binary!)
```

Note: `06-skills/resident-agent-core.md` and `resident-agent-modes/*` are **internal resident
prompts** served via MCP (`get_resident_prompt`) — they should NOT ship as Claude skills.
Ship only the two agent-facing skills.

### 2.1 `plugin/.claude-plugin/plugin.json`

```json
{
  "name": "altevra",
  "version": "0.5.0",
  "description": "Altevra second brain for Claude Code — persistent memory, semantic recall, wiki, skills and proactive context via the local Altevra MCP server and hooks",
  "author": {
    "name": "Pavle Anđelković / Imperium Tech",
    "email": "ceoimperiumprojects@gmail.com",
    "url": "https://github.com/ceoimperiumprojects"
  },
  "homepage": "https://github.com/ceoimperiumprojects/altevra",
  "repository": "https://github.com/ceoimperiumprojects/altevra",
  "license": "UNLICENSED",
  "keywords": ["memory", "second-brain", "mcp", "context", "agent-os", "local-first"]
}
```

(`license`: repo is "commercial license required" — `UNLICENSED` or a custom SPDX expression;
decide before tagging.)

### 2.2 `plugin/.mcp.json`

```json
{
  "altevra": {
    "command": "altevra",
    "args": ["serve", "--vault", "${ALTEVRA_VAULT:-${HOME}/.altevra/vault}"],
    "env": {
      "ALTEVRA_DB": "${ALTEVRA_DB:-${HOME}/.altevra/altevra.db}"
    }
  }
}
```

Open questions for this block (resolve before shipping):
- `${VAR:-default}` fallback syntax is **unverified** in Claude Code env expansion — if
  unsupported, ship `"args": ["serve"]` and make `serve` default its vault from
  `~/.altevra/config.toml` instead of the CLI flag (cleaner anyway; today `--vault` defaults
  to `.` which is wrong for a globally-installed plugin).
- `command: "altevra"` requires the binary on PATH (the known PATH issue from the Decisions
  log). The plugin README + an optional `/altevra-setup` command must cover the symlink
  (`~/.local/bin/altevra`). A plugin cannot bundle a 20MB+ platform-specific Rust binary
  sanely — prerequisite stays external (see §3).

### 2.3 `plugin/hooks/hooks.json`

Mirrors the currently-working `~/.claude/settings.json` wiring, made async where capture-only:

```json
{
  "description": "Altevra omniscient recorder — captures session lifecycle, prompts and tool use into the local brain (~/.altevra/altevra.db)",
  "hooks": {
    "SessionStart": [
      { "hooks": [ { "type": "command", "command": "altevra hook-handle session_start --tool claude-code", "timeout": 5 } ] }
    ],
    "UserPromptSubmit": [
      { "hooks": [ { "type": "command", "command": "altevra hook-handle user_prompt_submit --tool claude-code", "timeout": 5, "async": true } ] }
    ],
    "PreToolUse": [
      { "matcher": "*", "hooks": [ { "type": "command", "command": "altevra hook-handle pre_tool_use --tool claude-code", "timeout": 5, "async": true } ] }
    ],
    "PostToolUse": [
      { "matcher": "*", "hooks": [ { "type": "command", "command": "altevra hook-handle post_tool_use --tool claude-code", "timeout": 5, "async": true } ] }
    ],
    "Stop": [
      { "hooks": [ { "type": "command", "command": "altevra hook-handle session_end --tool claude-code", "timeout": 5 } ] }
    ]
  }
}
```

Notes:
- `--db` flag dropped in favor of the binary's default (`~/.altevra/altevra.db`) so the
  plugin stays machine-portable; keep `--db` only if the default isn't honored everywhere.
- SessionStart stays sync (it injects context); capture hooks go `async` per the hivemind
  pattern so they never block the agent.
- `altevra install-hooks` should learn to detect "plugin manages Claude Code hooks" and skip
  settings.json wiring for claude-code to avoid **double capture**.

### 2.4 Skills conversion table (06-skills → plugin/skills)

| Source | Plugin path | Frontmatter changes |
|---|---|---|
| `06-skills/altevra-core.md` | `plugin/skills/altevra-core/SKILL.md` | `slug→name`, `title` dropped (or merged into body H1), keep `description` + `version`; drop `tools:`/`tags:` (not part of SKILL.md schema, harmless but noisy) |
| `06-skills/altevra-agent-operations.md` | `plugin/skills/altevra-agent-operations/SKILL.md` | same; `allowed-tools:` list → keep (Claude Code supports `allowed-tools` in skill frontmatter) but verify tool names use the `mcp__altevra__*` form |
| `06-skills/resident-agent-core.md` + `resident-agent-modes/*` | **not shipped** | internal resident prompts, served via MCP |

Ideally generated at release time by a small `scripts/build-plugin.sh` (render from 06-skills,
never hand-fork — single source of truth stays in 06-skills/).

### 2.5 Root `.claude-plugin/marketplace.json`

```json
{
  "name": "altevra",
  "description": "Altevra — local-first second brain / Agent OS for AI tools",
  "owner": { "name": "Pavle Anđelković / Imperium Tech", "email": "ceoimperiumprojects@gmail.com" },
  "plugins": [
    {
      "name": "altevra",
      "description": "Persistent memory, semantic recall, wiki and proactive context for Claude Code via the local Altevra brain",
      "version": "0.5.0",
      "category": "productivity",
      "source": "./plugin",
      "homepage": "https://github.com/ceoimperiumprojects/altevra"
    }
  ]
}
```

User install flow once public:

```bash
claude plugin marketplace add ceoimperiumprojects/altevra
claude plugin install altevra@altevra
```

For an eventual entry in `anthropics/claude-plugins-official` (`external_plugins` style), the
entry would be the `git-subdir` form pinned to a release tag created by `claude plugin tag`:

```json
{ "source": { "source": "git-subdir", "url": "https://github.com/ceoimperiumprojects/altevra.git",
              "path": "plugin", "ref": "altevra--v0.5.0", "sha": "<commit>" } }
```

### 2.6 Install prerequisites note (must go in plugin README + marketplace description)

> **Prerequisite: the `altevra` binary.** This plugin is a thin shell over the Altevra Rust
> binary — it does NOT bundle it. Before enabling:
> 1. Install the binary (cargo build / release download / npm wrapper — see §3) and ensure
>    `altevra` is on PATH (e.g. `ln -s <build>/target/release/altevra ~/.local/bin/altevra`).
> 2. Run `altevra init` (or `altevra doctor`) once to create `~/.altevra/` (db, config, vault).
> 3. Restart Claude Code; the MCP server and hooks activate automatically.
> Without the binary, hooks fail silently (5s timeout) and the MCP server won't start.

---

## 3. npm-wrapper feasibility sketch (binary distribution — tradeoffs only)

Goal: `npm i -g altevra` (or `npx altevra`) gets the Rust binary onto PATH, which makes the
plugin's `command: "altevra"` Just Work. Three options:

### Option A — napi-rs (Rust → native Node addon)
- **How:** compile crates as a `.node` addon via napi-rs; npm package per-platform via
  `@altevra/cli-<platform>` optionalDependencies (the napi-rs CLI scaffolds this).
- **Pros:** first-class npm citizen; no postinstall network fetch (corporate-proxy friendly);
  napi-rs automates the per-platform publish matrix.
- **Cons:** WRONG SHAPE for Altevra — Altevra is a standalone CLI/daemon (serve, watch, brain),
  not a library called from Node. Wrapping a whole CLI in an addon means a Node host process
  forever in the loop (slower startup per hook call — hooks fire on every tool use), plus
  invasive build changes in crates/. Highest effort, least fit.

### Option B — postinstall-download (the esbuild/biome pattern)
- **How:** tiny JS package whose `postinstall` (or lazy first-run) downloads the right
  prebuilt binary from GitHub Releases into the package dir and shims `bin: altevra`.
  Modern variant (esbuild-style): per-platform binaries as `optionalDependencies` npm packages
  → no network fetch at all beyond npm itself.
- **Pros:** standard, proven, zero Rust-code changes; `npx altevra` works; the
  optionalDependencies variant avoids flaky postinstall scripts and works offline-mirrored.
- **Cons:** needs CI release matrix (linux-x64/arm64, darwin-x64/arm64, windows); npm package
  version must track binary releases; postinstall scripts are increasingly distrusted
  (`npm --ignore-scripts` breaks the naive variant — prefer optionalDependencies).
- **Fit: BEST.** Smallest delta, matches how every serious Rust CLI ships on npm.

### Option C — cargo-binstall pointer (no npm at all)
- **How:** publish GitHub Releases with binstall-compatible naming + `[package.metadata.binstall]`
  in Cargo.toml; users run `cargo binstall altevra` (falls back to `cargo install`). README
  one-liner: `curl … | sh` installer script as the non-Rust path.
- **Pros:** zero new packaging surface; Rust-native; binstall reuses the same GitHub Release
  artifacts Option B needs anyway.
- **Cons:** assumes a Rust-ish audience; no `npx` story; doesn't help the "plugin install
  should be one command" UX for non-dev users.

**Recommendation (prep stance):** build the GitHub Releases CI matrix once — it feeds both
B and C. Ship C immediately (free), add B (optionalDependencies variant) when the plugin goes
public to non-Rust users. Skip A unless Altevra ever needs an embeddable Node API.

---

## 4. Pre-publish checklist (when Pavle green-lights)

- [ ] Decide vault/db defaulting in `serve` (config-file default instead of `--vault .`)
- [ ] Verify `${VAR}` / `${VAR:-default}` expansion behavior in plugin `.mcp.json`
- [ ] `scripts/build-plugin.sh`: render 06-skills → plugin/skills with frontmatter conversion
- [ ] De-dupe hooks: plugin hooks vs `install-hooks` settings.json wiring (double-capture guard)
- [ ] License field decision (UNLICENSED vs custom)
- [ ] `claude plugin tag` release flow + sha-pinned marketplace entry
- [ ] Test locally: `claude plugin marketplace add /home/pavle/projekti/ai-tooling/altevra` →
      `claude plugin install altevra@altevra` → fresh session → hooks fire + MCP tools listed

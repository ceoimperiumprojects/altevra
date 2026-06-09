# Tool Register / Capabilities Layer — Assessment

**Date:** 2026-06-09
**Branch:** s0-foundation
**Assessor:** Claude (Sonnet 4.6, subagent)
**Scope:** Audit existing capability registry; enumerate Pavle's tool inventory; design the gap.

---

## 1. What exists today (verified against real DB + source)

### 1.1 Schema (migration 023_capability.sql)

Four tables landed in migration 023:

| Table | Purpose | Key columns |
|-------|---------|-------------|
| `adapter_dossiers` | Per-AI-tool capability matrix | `tool_name` (UNIQUE), `support_tier`, `surfaces` (JSON), `hook_events_supported` (JSON), `skill_format`, `install_targets` |
| `capability_records` | Honest can/cannot/unverified ledger | `actor`, `capability_key`, `support` (supported/unsupported/unverified/fallback), `evidence_ref` (REQUIRED when supported), `verification_method` |
| `skill_proposals` | Skill-factory output queue | `dedup_hash` (UNIQUE), `proposed_slug`, `proposed_body`, `occurrences`, `target_agents`, `status` |
| `capability_grants` | Cross-agent permission grants | `grantee`, `subject_kind` (skill/capability), `subject_ref`, `trust_level`, `requires_approval`, `approval_ref`, `status` |

Also: `installed_components` gained `capability_state` + `last_verified_at` columns via ALTER TABLE in the same migration.

### 1.2 Repository layer (capability.rs)

`crates/altevra-db/src/repositories/capability.rs` implements three repos:

- **`CapabilityRecordsRepository`**: upsert (T7 honesty: rejects `supported` without `evidence_ref`), get by (actor, key).
- **`SkillProposalsRepository`**: propose (T12 dedup by `dedup_hash` — same workflow proposes once, increments `occurrences`), occurrences query.
- **`CapabilityGrantsRepository`**: create_pending (derives `requires_approval` from TrustLevel, not trusted from caller), approve (T9 presence-gate: install/execute grants CANNOT be granted without a non-empty `approval_ref`), revoke, get, list.

These repos have hermetic unit tests that verify the business rules pass (T7, T12, T9).

### 1.3 MCP surface (tools_capabilities.rs + server.rs)

The `get_capabilities` MCP tool (line 253 in server.rs, handler in tools_capabilities.rs) reads a **static JSON file** at `~/.altevra/state/capabilities.json`. If that file does not exist, it returns a hardcoded default:

```json
{
  "adapters": ["claude-code", "codex", "cursor", "antigravity"],
  "skills": [],
  "hooks": ["session_start", "session_end", "on_error"],
  "mcp_tools": 22,
  "cli_commands": 15
}
```

This is **not backed by the DB tables**. The MCP tool does NOT query `adapter_dossiers` or `capability_records`. It reads/writes a flat file.

### 1.4 Real DB state (canonical /home/pavle/.altevra/altevra.db)

```
sqlite3 /home/pavle/.altevra/altevra.db ".tables"
```

Confirmed tables: `adapter_dossiers`, `capability_records`, `capability_grants`, `skill_proposals` all exist. Querying `adapter_dossiers` and `capability_records` returns zero rows — **the tables are empty**. The schema exists; no data has been seeded.

### 1.5 Bootstrap packet (tools_bootstrap.rs + packet.rs)

`get_agent_bootstrap_packet` MCP tool builds an `AgentBootstrapPacket` via `BootstrapBuilder`:
- `tool_name`, `project`, `altevra_version`
- `skill_freshness` (checks vault/06-skills for altevra-core skill version)
- `setup_status` (placeholder)
- `last_updates`, `warnings`, `recommended_next_action`
- Does NOT include: tool register / capability list / available-tools-to-invoke.

### 1.6 Critical gap: no "TOOL" concept distinct from "ADAPTER"

The schema models *AI agent adapters* (claude-code, codex, cursor, hermes) in `adapter_dossiers`, not *invocable tools* (Imperium Crawl, chatgpt-py, phone-use, etc.). There is no table or concept for:
- A tool Pavle can invoke (not an AI agent, but a capability like "browser automation via imperium-crawl")
- Its kind (skill/CLI/MCP-server/phone/script/web-service)
- How it is invoked (skill slug, binary path, API URL)
- What it can/cannot do (capability vector per tool)

The `~/.imperium/capabilities/manifest.yaml` has a `tools:` section listing agents and dev-runtime binaries, but this is outside Altevra and not imported.

---

## 2. Pavle's actual tool inventory (candidate seed)

Sources read: `~/.claude/skills/` (176 dirs), `~/.imperium/capabilities/{claude.yaml,hermes.yaml,manifest.yaml}`, CLAUDE.md, project context.

### 2.1 Priority tools for the register (Pavle's explicit list)

| name | kind | how_invoked | what_it_does | can | cannot | status |
|------|------|-------------|-------------|-----|--------|--------|
| `imperium-crawl` | cli | `imperium-crawl <command>` via shell or browser-automation skill | 28 tools, web scraping, browser automation, interact workflows, 466 tests — v2.3.1 stable | scrape, interact, screenshot browser, auth flows, workflow replay | can't run as MCP server today | can |
| `chatgpt-py` | cli+playwright | `chatgpt` CLI (python); skill: `/chatgpt-py` | Drives ChatGPT web UI via Playwright: GPT-4o, DALL-E 3, file upload/download, no API key needed | text generation, image gen (DALL-E 3), file analysis | requires active session; rate-limited by web UI | can |
| `notebooklm` | python-api | `notebooklm` CLI (notebooklm-py v0.3.4); skill: `/notebooklm` | Google NotebookLM automation: create notebooks, add sources, generate podcasts/summaries, download | all NLM artifact types incl podcast, summary, briefing doc | requires logged-in Google session; slow | can |
| `phone-use` | adb+ssh | `$PF <command>` (phone_fast.sh) via ADB WiFi 192.168.1.146:5555; skill: `/phone-use` | Android phone (Samsung Galaxy A14) full control: tap, swipe, type, screenshot, launch apps, file transfer | all phone UI interactions, social media posting, reading screen | macOS-style gestures; no root needed but limited to UI | can |
| `browser-automation` | skill+cli | `/browser-automation` skill → `imperium-crawl interact` | Multi-step browser flows, login, API key extraction, auth automation | dynamic auth flows, form fill, page navigation | not headless for captcha | can |
| `computer-use` | cli | `cu <command>` (cu script v3.1: xdotool+maim+wmctrl+tesseract+Python) | X11 desktop control: screenshot, click, type, OCR, window mgmt, human-motion simulation | all desktop UI, social posting via desktop, form filling | macOS-only CUA driver (ydotool unverified) | can |
| `transcribe` | cli | faster-whisper + yt-dlp; skill: `/transcribe <url-or-path>` | Audio/video transcription: YouTube subs, local files (m4a/mp3/wav/mp4), any yt-dlp URL | YouTube, local files, TikTok/Instagram/Twitter media | live audio; very long videos slow | can |
| `graphify` | skill+python | `/graphify <path>` skill → Python pipeline | Codebase/docs → knowledge graph: HTML viz, GraphRAG JSON, GRAPH_REPORT.md. Tree-sitter AST, community detection | code, docs, PDFs, images, videos → graph; MCP server mode | not semantic edge inference (AST-only today) | can |
| `hermes` | binary | `/home/pavle/.local/bin/hermes` | Command center AI agent: Telegram gateway, cron, kanban, memory, MCP server, multi-profile | orchestration, messaging, scheduling, delegating | long Rust coding tasks (delegates to claude) | can |
| `codex` | binary | `/home/pavle/.npm-global/bin/codex` | OpenAI Codex CLI: coding agent with big context, ChatGPT Plus backed, hooks, skills | deep coding tasks, PR review, large context (full repo) | cron, messaging, Telegram | can |
| `cursor` | binary | `/home/pavle/.local/bin/cursor` | Cursor agent CLI: AI coding tool, DB at `~/.cursor/ai-tracking/ai-code-tracking.db` | code editing, AI code tracking | import to Altevra needs cursor-cli adapter (v0.5 roadmap) | can |
| `imperium-cloud` | api-server | HTTP to localhost imperium-cloud PM2 process; skill: `/imperium-cloud` | Unified infra API: 17+ free cloud providers (AI chat, GPU compute, VM, browser, storage, jobs) | AI inference free, GPU via Colab/Kaggle, VM on Oracle/GCloud, browser automation | requires local PM2 process running; some providers unreliable | unverified |
| `vm-deploy` | skill | `/vm-deploy` | Deploy to Oracle Cloud VM (138.2.177.91) | VM deployment, service management | SSH key required | can |
| `vm-up` | skill | `/vm-up` | Ensure VM is up and healthy | VM health check, restart | requires Tailscale/SSH | can |
| `content-pipeline` | skill | `/content-pipeline` | LinkedIn/social content generation and scheduling pipeline | content creation, scheduling, posting via phone-use | requires phone-use for posting | can |

### 2.2 Additional skills worth registering (second tier)

`/home/pavle/.claude/skills/` has 176 entries. Notable categories:
- **Coding**: `tdd`, `code-review`, `simplify`, `harden`, `optimize`, `scaffold-exercises`
- **Research**: `deep-research`, `find-docs`, `grill-with-docs`, `competitor-analysis`, `content-research`
- **GSD workflow**: `gsd:*` family (30+ skills: `gsd:do`, `gsd:plan`, `gsd:ship`, etc.)
- **SEO**: `seo`, `seo-audit`, `seo-content`, etc. (15+ skills)
- **Imperium infra**: `imperium-cockpit`, `imperium-network`, `slusalice`
- **Writing**: `writing-beats`, `writing-fragments`, `writing-shape`, `edit-article`

Federated via `~/.imperium/skills/shared/`: imperium-crawl, imperium-network, linkedin, phone-use, publish, slusalice, tunia-content, vm-deploy, vm-up.

### 2.3 AI agent adapters (already modeled in adapter_dossiers, just unseeded)

| agent | binary | hook_protocol | support_tier |
|-------|--------|---------------|-------------|
| claude | `/home/pavle/.npm-global/bin/claude` | claude-style (6 events) | native |
| hermes | `/home/pavle/.local/bin/hermes` | hermes-style (5 events) | native |
| codex | `~/.local/bin/codex` | generic-stdin-json | partial |
| cursor | `~/.local/bin/cursor` | generic-stdin-json (v1 schema) | partial |
| gemini | `~/.npm-global/bin/gemini` | generic-stdin-json | manifest-only |
| opencode | `~/.local/bin/opencode` | generic-stdin-json | manifest-only |

---

## 3. Design gap — what needs to be built

### 3.1 Missing: `tool_records` table (Migration 035)

The existing `adapter_dossiers` models *AI coding agents* (claude, codex, cursor). It has no place for *invocable tools* (phone-use, imperium-crawl, chatgpt-py, etc.) which are not AI adapters — they are capabilities an agent invokes.

**New table proposal:**

```sql
-- 035_tool_records.sql
CREATE TABLE IF NOT EXISTS tool_records (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL UNIQUE,         -- e.g. "imperium-crawl", "phone-use"
    kind            TEXT NOT NULL,                -- skill | cli | python-api | mcp-server | web-service | adb
    display_name    TEXT NOT NULL,
    description     TEXT NOT NULL,
    invocation      TEXT NOT NULL DEFAULT '{}',  -- JSON: {method, path, skill_slug, binary, api_url}
    can_do          TEXT NOT NULL DEFAULT '[]',  -- JSON: string[] of capability keys
    cannot_do       TEXT NOT NULL DEFAULT '[]',  -- JSON: string[] of known limitations
    unverified      TEXT NOT NULL DEFAULT '[]',  -- JSON: string[] of unverified capabilities
    requires_session TEXT NOT NULL DEFAULT '{}', -- JSON: {auth_type, session_path, check_cmd}
    status          TEXT NOT NULL DEFAULT 'active', -- active|inactive|unverified|deprecated
    last_verified_at TEXT,
    provenance      TEXT NOT NULL DEFAULT '{"origin":"system_seed"}',
    categories      TEXT NOT NULL DEFAULT '[]',  -- JSON: ["automation","browser","content",...]
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
    updated_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE INDEX IF NOT EXISTS idx_tool_records_kind ON tool_records(kind);
CREATE INDEX IF NOT EXISTS idx_tool_records_status ON tool_records(status);
```

**Why not reuse `adapter_dossiers`:** `adapter_dossiers.tool_name` is scoped to AI coding agents (claude-code, codex, cursor, hermes, antigravity). Mixing phone-use and imperium-crawl in there pollutes the adapter concept and breaks the `support_tier` (native/partial/fallback_only) semantics. The two tables serve different consumers: adapter_dossiers is read by `install_hooks` to wire capture; tool_records is read by agents to know what tools they can invoke.

### 3.2 Missing: CLI surface (`altevra capability list` + `altevra tool list`)

Currently `get_capabilities` MCP returns a static JSON file. No CLI command lists tools.

**Needed CLI additions** in `crates/altevra-cli/src/commands/`:

```
altevra capability list                    # list capability_records (actor, key, support)
altevra capability record <actor> <key> <support> [--evidence <ref>]  # upsert a record
altevra tool list                          # list tool_records (name, kind, status)
altevra tool register <name> --kind <kind> --description <desc> ...   # insert/upsert
altevra tool seed                          # seed the predefined inventory (idempotent)
```

### 3.3 Missing: bootstrap packet includes tool register

`get_agent_bootstrap_packet` currently returns skill freshness + setup status + last updates. It does NOT include the tool register. Every agent session starts blind to what tools are available.

**Extend `AgentBootstrapPacket`** (packet.rs + bootstrap builder):

```rust
pub struct AgentBootstrapPacket {
    // ... existing fields ...
    pub available_tools: Vec<ToolSummary>,   // NEW: compact list from tool_records
    pub capability_summary: CapabilitySummary,  // NEW: actor's can/cannot/unverified
}

pub struct ToolSummary {
    pub name: String,
    pub kind: String,
    pub description: String,
    pub invocation_hint: String,  // "skill:/phone-use", "cli:chatgpt", etc.
    pub status: String,           // active|unverified
}
```

This is the Hivemind "SessionStart RULES/GOALS injection" pattern (docs/research/hivemind/04-integration-mcp-rules.md §6.1 item 1) applied to the tool inventory. Hivemind pushes rules/goals as a text block; Altevra should push a compact tool register as structured JSON in the bootstrap packet AND as a rendered text block in `build_system_prompt`.

### 3.4 Missing: `get_capabilities` backed by real DB

`handle_get_capabilities` (tools_capabilities.rs:45-63) reads a static JSON file. It should query:

```sql
SELECT * FROM tool_records WHERE status = 'active' ORDER BY kind, name;
SELECT * FROM capability_records WHERE actor = ? ORDER BY capability_key;
SELECT tool_name, support_tier, surfaces FROM adapter_dossiers ORDER BY tool_name;
```

Extend the handler to accept an optional `actor` arg and return a structured response:
```json
{
  "tools": [...],           // from tool_records
  "adapter_dossiers": [...], // from adapter_dossiers
  "capability_records": [...] // from capability_records, filtered by actor
}
```

### 3.5 Missing: `altevra tool seed` command with predefined inventory

The seed data for the 15 priority tools listed in Section 2.1 needs an idempotent seed command. This avoids manual insertion and ensures every fresh DB has the canonical inventory.

Pattern: a static `const SEED_TOOLS: &[ToolSeedEntry]` in the CLI crate, applied via upsert on `name` (ON CONFLICT DO UPDATE). No migration — seed is applied by the CLI, not by the DB migrator.

### 3.6 Missing: `build_system_prompt` tool section

`handle_build_system_prompt` (tools_prompts.rs) currently renders: safety layer, Altevra rules, tool behavior, project instructions, skills. It does NOT include an "available tools" section.

Add a layer between "skills" and "output protocol":

```
## Available Tools (Altevra Tool Register)

You can invoke these tools — do not wander or guess what's available:

| name | kind | how to invoke | status |
|------|------|---------------|--------|
| imperium-crawl | cli | browser-automation skill or direct CLI | active |
| phone-use | adb | /phone-use skill | active |
| chatgpt-py | playwright | /chatgpt-py skill | active |
| ... | ... | ... | ... |

Invoke unverified tools only after checking with Altevra (`report_capability_gap` if missing).
```

This is the "no wandering" guarantee Pavle asked for: every agent session knows exactly what tools exist.

---

## 4. Integration with Hivemind SessionStart pattern

From `docs/research/hivemind/04-integration-mcp-rules.md §3.2`:

Hivemind's `context-renderer.ts` injects a `=== HIVEMIND RULES ===` + `=== HIVEMIND GOALS ===` block at SessionStart for hook-capable tools (Claude, Cursor, Hermes) and provides `hivemind context` CLI for hook-less tools. Two-layer injection defense: reject newlines at write-time, sanitize at render-time.

**Altevra's equivalent should be:**

1. **Hook-capable (Claude Code):** `hook_handle session_start` emits `additionalContext` that includes a compact `=== ALTEVRA TOOL REGISTER ===` block (N active tools, formatted as name: description). Rendered by a shared `render_tool_register_block(db_path) -> String` function in `altevra-core`. Degrades to `""` on any error.

2. **Codex (TUI-visible additionalContext):** Per Hivemind's documented lesson (`04-integration-mcp-rules.md §6.1 item 3`), Codex's additionalContext is user-visible and clobbers the TUI. For Codex, skip the tool register in additionalContext; it's available via `altevra capability list` CLI or MCP `get_capabilities`.

3. **Hermes, Cursor:** Include in session_start hook as additionalContext (not user-visible).

4. **All tools:** `altevra context` CLI command (new, mirrors `hivemind context`) renders the current tool register + active goals to stdout for tools without session_start hook support.

---

## 5. Extension roadmap (S6, per PLAN.md)

PLAN.md §S6 explicitly scopes: "Add the missing CLI/MCP surface + tests to record/query can/cannot/unverified capabilities and grants. Imperium Crawl wired as ONE concrete capability/connector." This assessment provides the concrete design for what S6 builds:

| S6 item | Migration/file | Status |
|---------|---------------|--------|
| `tool_records` table | new `035_tool_records.sql` | missing |
| `ToolRecordsRepository` | new in `crates/altevra-db/src/repositories/` | missing |
| `altevra tool list/register/seed` CLI | new commands in altevra-cli | missing |
| `get_capabilities` DB-backed | extend `tools_capabilities.rs` | partial (reads static file) |
| `altevra capability list/record` CLI | new commands in altevra-cli | missing |
| Tool register in bootstrap packet | extend `packet.rs` + `BootstrapBuilder` | missing |
| Tool section in `build_system_prompt` | extend `prompts.rs` + `tools_prompts.rs` | missing |
| SessionStart hook emits tool register block | extend `hook_handle.rs` + `commands/turn.rs` | missing |
| `altevra context` CLI | new command | missing |
| Imperium Crawl seeded + grant created | `altevra tool seed` + `altevra capability record` | missing |

---

## 6. Verified facts vs assumptions

**Verified (command output as evidence):**
- `adapter_dossiers`, `capability_records`, `skill_proposals`, `capability_grants` tables exist in real DB (`.tables` query confirmed).
- All four tables are empty — zero rows in adapter_dossiers, capability_records (queries returned no output = empty result).
- `tool_installations` table exists with correct schema (no tool_records).
- `get_capabilities` MCP tool reads static JSON file, not DB tables (source code verified).
- Bootstrap packet contains NO tool register (source code verified, `AgentBootstrapPacket` struct in packet.rs has no tools field).
- 176 skills in `~/.claude/skills/` (ls | wc -l confirmed).
- `~/.imperium/capabilities/manifest.yaml` exists with full tool/agent inventory (read confirmed).
- `~/.imperium/capabilities/claude.yaml` and `hermes.yaml` have detailed can/cannot lists.

**Assessed (not individually runtime-verified):**
- Capability repo business rules (T7, T12, T9) — verified via unit tests in capability.rs, not by running them against real DB.
- Skill invocation methods — read from SKILL.md files, not smoke-tested.

---

## 7. Files to create/modify for S6 tool register work

```
crates/altevra-db/migrations/035_tool_records.sql        (NEW)
crates/altevra-db/src/repositories/tool_records.rs       (NEW)
crates/altevra-db/src/repositories/mod.rs                (EXTEND: pub mod tool_records)
crates/altevra-cli/src/commands/capability.rs            (NEW: list + record subcommands)
crates/altevra-cli/src/commands/tool.rs                  (NEW: list + register + seed)
crates/altevra-cli/src/commands/context.rs               (EXTEND: tool register block output)
crates/altevra-mcp/src/tools_capabilities.rs             (EXTEND: query DB instead of file)
crates/altevra-bootstrap/src/packet.rs                   (EXTEND: add available_tools field)
crates/altevra-bootstrap/src/lib.rs                      (EXTEND: BootstrapBuilder.tools())
crates/altevra-core/src/prompts.rs                       (EXTEND: PromptInput.available_tools)
crates/altevra-mcp/src/tools_prompts.rs                  (EXTEND: render tool register layer)
crates/altevra-mcp/src/tools_bootstrap.rs                (EXTEND: load tool_records from DB)
crates/altevra-brain/src/jobs.rs                         (optional: periodic tool verify job)
```

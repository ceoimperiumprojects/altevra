<!-- ───────────────────────────────────────────────────────────────────────
  ⚠️ PARTIALLY SUPERSEDED (2026-06-01)

  The storage-engine sections of this document (Postgres / pgvector) are
  SUPERSEDED. Altevra P0 is **SQLite local-first canonical**; Postgres/pgvector
  is a future opt-in cloud adapter only. See `docs/architecture/RECONCILIATION.md`
  (R10) and the deep architecture sections in
  `docs/architecture/ALTEVRA_ARCHITECTURE_WORKING_DRAFT.md`.

  Read this V5 doc for product/feature vision; for the binding P0 data/storage
  contracts use the reconciled architecture docs, not the Postgres text here.
─────────────────────────────────────────────────────────────────────── -->

# Altevra Production Architecture v5

> CLI-first. Rust-first. Local-first. MCP-compatible.  
> Agent OS for memory, research, tasks, goals, skills, capabilities, hooks, sync, and self-improving context.

## 0. One-line Definition

Altevra is a local-first, Rust-powered Agent OS that gives AI tools a shared, fresh, searchable, auditable context layer.

It provides: memory, project context, tasks, goals, decisions, skills, capabilities, secrets, hooks, research, scraping, event history, last updates, connector sync, prompt generation, tool setup automation, MCP tools, CLI commands, and self-observing agent workflows.

Core promise: **Every agent always knows what changed, what matters, and how to use Altevra.**

## 1. Core Product Principle

Altevra must not only store context. It must also collect, clean, structure, search, distribute, version, explain, sync into tools, observe usage, and tell agents what changed.

The agent should ask Altevra: **“What changed since my last session, and what should I know before working?”**

## 2. v5 Critical Systems

1. Last Updates Feed
2. Event Stream
3. Change Journal
4. Hook System
5. Tool Setup Automation
6. Adapter Capability Matrix
7. Skill + Hook Sync
8. Agent Bootstrap Protocol
9. Context Freshness Protocol
10. System Prompt Generator
11. Secrets + Permissions Layer
12. Install/Verify/Repair Flow
13. Managed Config Registry
14. Update Diff System
15. Tool-specific Setup Packs
16. Production Observability

## 3. Mental Model

Something changes → event recorded → affected context updated → agent receives fresh update → agent acts better → action logged → observer learns patterns → skills/prompts/hooks improve.

This is a living context system, not notes and not RAG soup.

## 4. Main Architecture

```txt
AI tools: Claude Code / Codex / Cursor / Aider / OpenCode
        │ MCP / CLI / Hooks / Files
        ▼
Altevra Interface Layer: CLI | MCP Adapter | REST Internal API | Hook Runner
        ▼
Altevra Core Engine: Rust core for memory, tasks, goals, skills, hooks, events
        ▼
Engines: Memory | Updates | Skills | Hooks | Research
        ▼
Postgres + pgvector + Vault: docs, chunks, events, updates, tasks, goals, skills, sessions
```

## 5. Rust-first Stack

- Language: Rust
- Runtime: Tokio
- API: Axum
- CLI: clap
- Database: Postgres
- Vector: pgvector
- DB access: sqlx
- Serialization: serde
- Config: TOML/YAML via config crate
- Markdown: pulldown-cmark + frontmatter parser
- File watching: notify
- HTTP: reqwest
- HTML parsing: scraper
- Secrets: keyring crate + encrypted local store
- Logging: tracing
- Errors: thiserror / anyhow

Architecture rule: **CLI is primary. MCP is adapter. REST is internal/service interface. Dashboard is later.**

No duplicate logic. No Claude-specific hardcoding inside core.

## 6. Last Updates System

Agents need to know what changed recently.

### Purpose

Answer what changed since last 10 minutes, last hour, last 24h, last session, last time this agent ran, or last time this project was touched.

### Update types

document_changed, document_indexed, skill_updated, skill_installed, skill_drift_detected, task_created, task_updated, task_completed, goal_created, goal_updated, project_status_changed, decision_saved, research_saved, research_synthesized, insight_created, hook_installed, hook_failed, adapter_synced, config_changed, session_started, session_ended, capability_added, connector_synced, secret_changed, error_logged, review_item_created.

### events table

- id uuid primary key
- event_type text not null
- project_id uuid null
- actor_type text not null
- actor_id text null
- source text not null
- entity_type text null
- entity_id text null
- title text not null
- summary text null
- payload jsonb not null
- sensitivity text not null
- created_at timestamptz not null
- processed_at timestamptz null
- status text not null

### update_feed table

Processed, agent-friendly events.

- id uuid primary key
- event_id uuid references events(id)
- project_id uuid null
- update_type text not null
- importance text not null: critical/high/medium/low/noise
- title text not null
- short_summary text not null
- agent_summary text null
- affected_entities jsonb
- recommended_agent_action text null
- visible_to_agents bool not null
- sensitivity text not null
- created_at timestamptz not null

### Rules

Every action logs an event. Only useful events become visible updates. Critical changes always show. Project agents see project updates first. Sensitivity gates apply.

### CLI

```bash
altevra updates
altevra updates --since 24h
altevra updates --project altevra
altevra updates --agent claude-code
altevra updates --important
altevra updates --json
altevra updates mark-read
```

### MCP tool

`get_last_updates(project, since, agent_id, importance_min)` returns update list plus warnings.

### Startup rule

Every meaningful agent session starts with skill version, last updates since last session, project context, active task, then work.

## 7. Change Journal

Human-readable summary stored in vault:

- `/10-insights/change-journal.md`
- `/10-insights/change-journal/YYYY-MM-DD.md`

CLI:

```bash
altevra journal today
altevra journal --project altevra
altevra journal generate --since 24h
```

## 8. Hook System

Universal Hook Spec → Adapter Renderer → Native Tool Hook Config.

Universal hook types:

- session_start / session_end
- before_tool_call / after_tool_call
- before_file_edit / after_file_edit
- before_command / after_command
- on_error
- on_skill_check
- on_context_request
- on_task_complete
- on_project_switch

Universal hook file: `/07-capabilities/hooks.yaml`.

Hook actions include check_skill_version, get_last_updates, get_project_context, start_session_log, end_session_log, summarize_session, emit_event, schedule_ingestion, create_pending_change, detect_secret_leak, create_review_item.

Execution model:

```bash
altevra hook run session_start --tool claude-code --project altevra --json "$PAYLOAD"
```

## 9. Hook Adapter System

Rust trait:

```rust
pub trait ToolAdapter {
    fn tool_name(&self) -> &'static str;
    fn detect(&self, repo_path: &Path) -> AdapterDetectionResult;
    fn render_instructions(&self, input: InstructionRenderInput) -> Result<Vec<GeneratedFile>>;
    fn render_skills(&self, skills: Vec<UniversalSkill>) -> Result<Vec<GeneratedFile>>;
    fn render_hooks(&self, hooks: UniversalHooks) -> Result<Vec<GeneratedFile>>;
    fn install(&self, plan: InstallPlan) -> Result<InstallResult>;
    fn verify(&self, repo_path: &Path) -> Result<VerifyResult>;
    fn repair(&self, repo_path: &Path) -> Result<RepairPlan>;
}
```

Adapter capability matrix at `/07-capabilities/agent-tools.yaml`. Be honest: unsupported tools get instructions/CLI fallback/wrapper, not fake hooks.

## 10. Auto-Setup System

One command should wire a tool:

```bash
altevra setup --tool claude-code --project altevra
altevra setup --dry-run --tool claude-code
altevra setup verify --tool claude-code
altevra setup repair --tool claude-code
```

Setup flow: detect repo/project/tool, load adapter/capabilities/skills/instructions/preferences, generate native files/hooks/MCP/fallbacks, check secrets/sensitivity, show dry-run diff, apply if approved, verify, emit event and update_feed item.

Managed file header required:

```html
<!-- ALTEVRA_MANAGED: true -->
<!-- source: /06-skills/altevra-core.md -->
<!-- generated_by: altevra -->
<!-- adapter: claude-code -->
<!-- version: 0.5.0 -->
<!-- checksum: sha256_here -->
<!-- generated_at: 2026-05-27T14:00:00Z -->
```

Never silently overwrite drifted files.

## 11. Setup Packs

Stored at `/15-generated/setup-packs/{tool}/` with manifest, generated-files, hooks, install-plan.json, verification.json.

## 12. Skill + Hook Freshness

Tables: skills, hooks, tool_installations, installed_components. Component statuses: current, outdated, drifted, missing, conflicted, unsupported.

CLI:

```bash
altevra setup status --tool claude-code --project altevra
```

## 13. Agent Bootstrap Protocol

1. Identify tool
2. Identify project
3. Call Altevra bootstrap
4. Check skill version
5. Check setup/hook status
6. Get last updates since last session
7. Get project context
8. Get active task/goal
9. Get warnings/conflicts
10. Start work

CLI:

```bash
altevra agent bootstrap --tool claude-code --project altevra --json
```

MCP: `get_agent_bootstrap_packet(tool_name, project, installed_skill_version, session_id)`.

## 14. System Prompt Engineering

Prompt layers: safety/sensitivity, Altevra rules, tool behavior, project instructions, current task/goal, last updates, skills, output protocol.

Priority: safety > explicit user instruction > source-of-truth > agent-specific > global Altevra rules > inferred preferences > research suggestions.

## 15. Tool-specific System Prompts

Claude Code: read Altevra skill, call MCP or CLI bootstrap, check updates, load task, use hooks/fallback, scoped edits/tests.

Codex: do not assume Claude hooks, use CLI fallback if native integration unavailable.

Antigravity: adapter-driven; never assume hook/config/skill/MCP support until dossier marks supported.

Cursor: rules + CLI fallback; do not manually edit managed rules.

## 16. Auto-Connection Strategy

`altevra connect --tool claude-code --project altevra` is alias to setup. Installs instruction file, skill file, MCP config, hook config, fallback scripts, managed headers, setup manifest, verification record as supported by adapter.

## 17. Secrets System

MVP: local encrypted store + OS keyring if available. `.env` only for development.

Commands:

```bash
altevra secrets set DEEPSEEK_API_KEY
altevra secrets get DEEPSEEK_API_KEY
altevra secrets list
altevra secrets delete DEEPSEEK_API_KEY
```

Secrets must never appear in generated prompts, skills, hooks, or config files. Hook files call Altevra CLI, which resolves secrets internally. Detect keys/tokens/private keys/DB URLs and block sync.

## 18. Production Data Model Additions

### tool_installations

id, tool_name, project_id, adapter_version, installed_at, last_verified_at, status, metadata jsonb.

### installed_components

id, installation_id, component_type, component_slug, installed_version, installed_path, checksum, status, last_checked_at.

### hooks

id, slug, version, source_file, checksum, status, created_at, updated_at.

### hook_runs

id, hook_slug, tool_name, project_id, payload jsonb, result jsonb, success, error_message, duration_ms, created_at.

### update_read_state

id, actor_type, actor_id, project_id, last_seen_event_id, last_seen_at.

## 19. Event-to-Update Pipeline

event inserted → classifier job → importance score → summary → update_feed item → affected agents/projects.

Critical: secret leak blocked, source-of-truth conflict, skill update required, hook failure, MCP unavailable, database migration needed. High: architecture changed, skill updated, deadline/status/decision. Medium: research synthesized/new task/adapter sync. Low: indexed/minor doc/routine. Noise: temp logs/duplicates.

## 20. Hook Setup Per Tool

- Claude Code: native hooks/skills/MCP/CLAUDE.md
- Codex: AGENTS.md + CLI fallback until researched
- Antigravity: no assumptions, adapter dossier first
- Cursor: `.cursor/rules/altevra.mdc` + CLI fallback
- Aider: `CONVENTIONS.md`, `.aider/commands/altevra.md`, fallback

## 21. Production CLI Command Map

Setup: `altevra init`, `altevra serve`, `altevra mcp start`, `connect`, `connect verify`, `connect repair`, `connect status`.

Updates: `altevra updates --since 24h|last-session --project ... --agent ... --json`, `mark-read`.

Hooks: `hook list`, `hook run`, `hook status`, `hook install`, `hook verify`.

Skills: `skill list`, `skill show`, `skill check --all`, `skill refresh`.

Agent: `agent bootstrap`, `agent status`, `agent instructions`.

Memory: `ingest`, `search`, `context`, `packet`.

Research: `research`, `scrape`, `synthesize`.

Maintenance: `doctor`, `eval run`, `cleanup --dry-run`, `journal today`.

## 22. MCP Tools v5

get_agent_bootstrap_packet, check_altevra_skill_version, get_altevra_skill, get_last_updates, mark_updates_read, get_project_context, get_context_packet, search_memory, get_source_of_truth, get_active_tasks, get_goals, save_task, update_task, save_decision, list_skills, get_skill, get_capabilities, report_knowledge_gap, report_capability_gap, create_review_item, run_hook, get_setup_status, request_skill_refresh.

## 23. Context Freshness Protocol

At start: bootstrap, skill check, setup/hook check, last updates, project context, tasks/goals. During work: ask Altevra, save decisions, report gaps, update tasks. End: summarize, emit session_end, update task, write updates.

## 24. Minimal First Build For v5

Do not build all of v5. Build infrastructure spine first.

v0.1 Target:

- Rust CLI
- Skill registry
- Last Updates system
- Events table
- Hook system skeleton
- Claude Code adapter skeleton
- Agent bootstrap packet
- MCP skill/update tools

Build order:

1. Rust workspace
2. CLI skeleton
3. Postgres migrations
4. events table
5. update_feed table
6. skill registry
7. skill version/checksum
8. hook registry
9. adapter trait
10. Claude Code adapter skeleton
11. connect --dry-run
12. updates command
13. agent bootstrap command
14. MCP tools: get_agent_bootstrap_packet, get_last_updates, check_altevra_skill_version
15. generated altevra-core skill
16. README

Do NOT build dashboard, all adapters, full observer brain, full research engine, Google Workspace, Slack, Linear, NotebookLM.

## 25. Rust Module Structure v5

```txt
crates/
  altevra-core/ config.rs errors.rs events.rs updates.rs security.rs
  altevra-cli/ main.rs commands/{init.rs,connect.rs,updates.rs,hook.rs,skill.rs,agent.rs,search.rs}
  altevra-db/ pool.rs migrations/ repositories/{events.rs,updates.rs,skills.rs,hooks.rs,installations.rs}
  altevra-skills/ parser.rs registry.rs version.rs checksum.rs renderer.rs drift.rs
  altevra-hooks/ registry.rs runner.rs actions.rs universal.rs
  altevra-adapters/ base.rs claude_code.rs codex.rs cursor.rs aider.rs antigravity.rs
  altevra-bootstrap/ packet.rs freshness.rs setup_status.rs
  altevra-mcp/ server.rs tools_bootstrap.rs tools_updates.rs tools_skills.rs tools_memory.rs
  altevra-vault/ parser.rs frontmatter.rs writer.rs
  altevra-memory/ ingestion.rs chunker.rs search.rs
  altevra-research/ pipeline.rs scraper.rs synthesis.rs
  altevra-secrets/ store.rs detector.rs redactor.rs
```

## 26. Agent Prompt: Altevra Core v5

Every tool gets a skill with mandatory startup: identify tool/project, check skill version, setup/hook status, last updates, project context, task/goal, warnings/conflicts, then act. If MCP unavailable, use CLI:

```bash
altevra agent bootstrap --tool {tool} --project {project} --json
altevra updates --project {project} --since last-session --json
altevra context --project {project} --json
```

Rules: check last updates, warn on outdated skills, use CLI fallback if hooks missing, prefer source-of-truth, never leak secrets, finish with session summary/task update/session_end.

## 27. First Overnight Agent Prompt

Build Altevra v5 Foundation. We are building Altevra in Rust: CLI-first, Rust-first, local-first, MCP-compatible, adapter-based, skill-versioned, hook-aware, update-feed aware.

### Must Build

1. Rust workspace with crates: altevra-core, altevra-cli, altevra-db, altevra-skills, altevra-hooks, altevra-adapters, altevra-bootstrap, altevra-mcp.
2. Postgres migrations: events, update_feed, skills, skill_installations, hooks, hook_runs, tool_installations, installed_components.
3. CLI commands: init, updates, skill list, skill check, hook list, hook run, connect --tool claude-code --project altevra --dry-run, agent bootstrap --tool claude-code --project altevra --json.
4. Skill system: parser, version parser, checksum generator, registry, example /06-skills/altevra-core.md.
5. Update system: event creation, update_feed creation, updates CLI output and JSON output.
6. Hook system: universal registry, runner skeleton, hook event logging, session_start/session_end.
7. Adapter system: ToolAdapter trait, Claude Code adapter skeleton, generated file representation, managed headers, dry-run install plan.
8. Bootstrap: packet struct, skill freshness check, setup status placeholder, last updates included.
9. MCP skeleton: get_agent_bootstrap_packet, get_last_updates, check_altevra_skill_version.
10. README explaining product, CLI, freshness, updates, hooks, and not-implemented-yet.

### Do Not Build

Dashboard, full memory ingestion, full research engine, Google Workspace, Slack, Linear, NotebookLM, all adapters, observer brain, synthesis engine.

### Rules

CLI is primary. MCP calls same core logic. Do not hardcode Claude outside Claude adapter. Generated files need managed headers. Never overwrite drifted files silently. Hooks universal first, native through adapter. Last updates through CLI and MCP. Every important action emits event. Keep modules clean and testable.

## 28. Final v5 Definition

Altevra v5 is not just a context database. It is a production-grade Agent OS with memory, updates, skills, hooks, tasks, goals, research, connectors, adapters, MCP, CLI, secrets, events, audit, bootstrap, freshness, and sync.

Most important idea: **Agents must know what changed before they work.**

Second: **Altevra must install and refresh the instructions that teach agents how to use Altevra.**

Third: **Hooks must be universal in Altevra, native only through adapters.**

Final sentence: **Altevra turns disconnected AI tools into one fresh, context-aware, self-updating agent system.**

## Pavle's extra implementation note

For model/AI storage inside the database, do not add an external API integration yet. It is enough for now that Hermes/Claude/Codex can attach to the project/database and execute inside it. API plumbing comes later.

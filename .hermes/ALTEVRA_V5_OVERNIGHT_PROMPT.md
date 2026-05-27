# Build Altevra v5 Foundation — Overnight Claude Task

REAL INTERACTIVE SESSION, no `-p`.

You are Claude Code working on Pavle's new project **Altevra**.

Repo: `/data/pavle/projekti/Altevra`
Spec file: `ALTEVRA_PRODUCTION_ARCHITECTURE_V5.md`

## Mission

Work all night on the v5 foundation. Do **not** build the whole product. Build the nervous system first:

- events
- update_feed
- skills
- hooks
- adapter skeleton
- bootstrap packet
- CLI/MCP foundation

## Mandatory Startup

1. Read `AGENTS.md`.
2. Read `ALTEVRA_PRODUCTION_ARCHITECTURE_V5.md`.
3. Inspect current repo state.
4. Create a concrete implementation plan in the terminal.
5. Then implement.

## Must Build

1. Rust workspace with crates:
   - `altevra-core`
   - `altevra-cli`
   - `altevra-db`
   - `altevra-skills`
   - `altevra-hooks`
   - `altevra-adapters`
   - `altevra-bootstrap`
   - `altevra-mcp`

2. Postgres migrations:
   - `events`
   - `update_feed`
   - `skills`
   - `skill_installations`
   - `hooks`
   - `hook_runs`
   - `tool_installations`
   - `installed_components`

3. CLI commands:
   - `altevra init`
   - `altevra updates`
   - `altevra skill list`
   - `altevra skill check`
   - `altevra hook list`
   - `altevra hook run`
   - `altevra connect --tool claude-code --project altevra --dry-run`
   - `altevra agent bootstrap --tool claude-code --project altevra --json`

4. Skill system:
   - universal skill parser
   - version parser
   - checksum generator
   - skill registry
   - example `06-skills/altevra-core.md`

5. Update system:
   - event creation
   - update_feed creation
   - updates CLI output
   - updates JSON output

6. Hook system:
   - universal hook registry
   - hook runner skeleton
   - hook event logging
   - `session_start` and `session_end` hooks

7. Adapter system:
   - `ToolAdapter` trait
   - Claude Code adapter skeleton
   - generated file representation
   - managed file headers
   - dry-run install plan

8. Bootstrap:
   - agent bootstrap packet struct
   - skill freshness check
   - setup status placeholder
   - last updates included in packet

9. MCP skeleton:
   - `get_agent_bootstrap_packet`
   - `get_last_updates`
   - `check_altevra_skill_version`

10. README:
   - what Altevra is
   - how to run CLI
   - how skill freshness works
   - how last updates work
   - how hooks work
   - what is not implemented yet

## Do NOT Build

- dashboard
- full memory ingestion
- full research engine
- Google Workspace connector
- Slack connector
- Linear connector
- NotebookLM connector
- all adapters
- full observer brain
- full synthesis engine
- external model/API integration

## Pavle-specific note

For model/AI in the database, **do not add API integration yet**. It is enough that Hermes/Claude/Codex can attach to the project/database and execute inside it. API layer comes later.

## Rules

- CLI is primary.
- MCP calls the same core logic.
- Do not hardcode Claude-specific behavior outside the Claude adapter.
- Generated files need managed headers.
- Never silently overwrite drifted files.
- Hooks must be universal first, native through adapters.
- Last updates must be available through CLI and MCP.
- Every important action emits an event.
- Keep modules clean and testable.
- Run `cargo fmt`, `cargo check`, and relevant tests before claiming done.
- Do not commit, push, deploy, contact anyone, or perform external side effects without Pavle approval.

## End-of-run output

Print:

1. What was implemented
2. Files changed
3. Commands/tests run
4. Current working state
5. Blockers
6. Next exact task for morning

<!-- ALTEVRA_MANAGED: true -->
<!-- source: 06-skills/altevra-core.md -->
<!-- generated_by: altevra -->
<!-- adapter: claude-code -->
<!-- version: 0.5.0 -->
<!-- checksum: 7fddebaa49cde5805652285dd6fa085c69a04d036936181b10cc90c3caf41e86 -->
<!-- generated_at: 2026-05-27T04:56:35.136594738+00:00 -->

# Altevra Core Skill

Version: 0.5.0

# Altevra Core — Agent Skill

**Version:** 0.5.0
**Required:** Yes — all agents must bootstrap with Altevra at session start.

## What is Altevra?

Altevra is a local-first Agent OS. It gives AI tools a shared, fresh, searchable, auditable context layer.

It provides: memory, project context, tasks, goals, decisions, skills, capabilities, hooks, research, event history, last updates, connector sync, prompt generation, and tool setup automation.

**Core promise:** Every agent always knows what changed, what matters, and how to use Altevra.

## Mandatory Session Startup Protocol

At the **start of every session**, execute:

```bash
altevra agent bootstrap --tool {tool_name} --project {project} --json
```

Or via MCP: `get_agent_bootstrap_packet(tool_name="{tool_name}", project="{project}")`

This returns:
- `skill_freshness` — are your skills up to date?
- `setup_status` — are hooks/managed files installed?
- `last_updates` — what changed since your last session?
- `warnings` — anything requiring immediate attention
- `recommended_next_action` — what to do first

## Quick CLI Reference

```bash
# What changed since last session?
altevra updates --project {project} --since last-session --json

# Are my skills current?
altevra skill check --all

# Run session startup hook
altevra hook run session_start --tool {tool_name} --project {project} --json

# Connect tool to project (dry-run first)
altevra connect --tool {tool_name} --project {project} --dry-run

# Get project context
altevra context --project {project} --json
```

## Rules You Must Follow

1. **Always check last updates** before working. Use bootstrap packet or `altevra updates`.
2. **Warn if any skill is outdated.** Run `altevra skill check --all` and alert user.
3. **Use CLI fallback** if MCP is unavailable: `altevra agent bootstrap --json`
4. **Never edit ALTEVRA_MANAGED files** manually. They are generated — edit source and re-run `altevra connect`.
5. **Save decisions** to Altevra: `altevra context save-decision --title "..." --content "..."`
6. **Finish sessions** with: `altevra hook run session_end --tool {tool_name} --project {project}`
7. **Report knowledge gaps**: If you don't know something about the project, use `altevra report-gap --description "..."`.

## Skill Version Check

At startup, verify your installed skill is current:

```bash
altevra skill check altevra-core --installed-version {your_version}
```

If outdated → run `altevra skill refresh altevra-core` or `altevra connect --tool {tool_name}`.

## MCP Tools Available

| Tool | Description |
|------|-------------|
| `get_agent_bootstrap_packet` | Full session bootstrap — call at every session start |
| `get_last_updates` | What changed? Since when? |
| `check_altevra_skill_version` | Am I running a fresh skill? |
| `get_project_context` | Current project state, goals, decisions |
| `search_memory` | Search vault documents |
| `save_decision` | Record a project decision |
| `get_active_tasks` | Current task list |
| `run_hook` | Run any registered hook |

## Context Freshness Protocol

**Start of session:**
1. `get_agent_bootstrap_packet` (or `altevra agent bootstrap --json`)
2. Check `skill_freshness` — if outdated, warn and suggest refresh
3. Check `last_updates` — read what changed
4. Check `active_task` — what are you working on?
5. Check `warnings` — any blockers?
6. Then start work.

**During work:**
- Before editing a file: check if it's `ALTEVRA_MANAGED`
- When you make a decision: save it via Altevra
- When you complete a task: update task status

**End of session:**
- Run `altevra hook run session_end`
- Confirm tasks are updated

## Not Implemented Yet (v0.1)

These are planned but not in v0.1:
- `altevra context` command
- `altevra search` command
- `altevra ingest` command
- Full memory search (vector/pgvector)
- Research/synthesis engine
- Google Workspace / Slack / Linear connectors
- Full task system with due dates and assignees
- Goal tree system
- Dashboard

Use what is available. Report gaps with `altevra report-gap` when you hit missing features.
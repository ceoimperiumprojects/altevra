<!-- ALTEVRA_MANAGED: true -->
<!-- source: 07-capabilities/agent-tools.yaml -->
<!-- generated_by: altevra -->
<!-- adapter: claude-code -->
<!-- version: 0.1.0 -->
<!-- checksum: 2a65ee1867ed87bf1ff36aafe41048370868a26fd825f96c3700e859309ad419 -->
<!-- generated_at: 2026-05-27T04:56:35.136494766+00:00 -->

# Altevra Context

Project: altevra

## Session Startup

At the start of every session, call:

```bash
altevra agent bootstrap --tool claude-code --project ${ALTEVRA_PROJECT} --json
```

Or via MCP: `get_agent_bootstrap_packet(tool_name="claude-code", project="${ALTEVRA_PROJECT}")`

## Quick CLI Reference

```bash
altevra updates --project ${ALTEVRA_PROJECT} --json          # What changed since last session
altevra skill check --all                                      # Are my skills fresh?
altevra hook run session_start --tool claude-code              # Run startup hook
altevra context --project ${ALTEVRA_PROJECT} --json          # Current project context
```

## Rules

- Check last updates before working.
- Warn if any skill is outdated.
- Use CLI fallback if MCP is unavailable.
- Never edit ALTEVRA_MANAGED files manually.
- Finish session with: `altevra hook run session_end --tool claude-code --project ${ALTEVRA_PROJECT}`

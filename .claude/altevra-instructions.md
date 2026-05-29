<!-- ALTEVRA_MANAGED: true -->
<!-- source: 07-capabilities/agent-tools.yaml -->
<!-- generated_by: altevra -->
<!-- adapter: claude-code -->
<!-- version: 0.1.0 -->
<!-- checksum: b51bc552c0d938782b0f15b9c5b7237462a97798145aa678413e4f8b7f0d16e1 -->

# Altevra Context

Project: (set ALTEVRA_PROJECT env var)

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

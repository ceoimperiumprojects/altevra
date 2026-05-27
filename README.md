# Altevra

Local-first Agent OS — skills, hooks, and context delivery for AI-assisted development.

## Status: v5 Foundation — CI-green

- 58 tests passing
- `cargo fmt --check` clean
- MCP server verified via stdio smoke

## Quick start

```bash
cargo build
./target/debug/altevra --help
```

## Core commands

| Command | What it does |
|---|---|
| `altevra init` | Scaffold `.altevra/` vault structure in current project |
| `altevra connect --tool claude-code --project <name>` | Install skills + hooks into `.claude/` |
| `altevra connect --tool claude-code --dry-run --json` | Preview without writing |
| `altevra skill list --vault .` | List skills in vault |
| `altevra skill check --all --vault .` | Check installed vs latest |
| `altevra agent bootstrap --tool claude-code --json` | Get session bootstrap packet |
| `altevra serve --vault .` | Start MCP JSON-RPC 2.0 stdio server |
| `altevra updates --json` | Get recent update feed |

## MCP mount (manual step)

**Option A — project-scoped `.mcp.json`** (preferred):
```json
{
  "mcpServers": {
    "altevra": {
      "command": "altevra",
      "args": ["serve", "--vault", "/path/to/your/vault"],
      "type": "stdio"
    }
  }
}
```
Place `.mcp.json` at your project root.

**Option B — global** (`~/.claude/mcp_config.json`, not `settings.json`):
```json
{
  "mcpServers": {
    "altevra": {
      "command": "altevra",
      "args": ["serve", "--vault", "/data/pavle/projekti/Altevra"],
      "type": "stdio"
    }
  }
}
```

Verify: `claude mcp list` — should show `altevra` connected.

Generated config reference: `15-generated/setup-packs/claude-code/mcp-config.json`

## Exposed MCP tools

| Tool | Purpose |
|---|---|
| `get_agent_bootstrap_packet` | Full bootstrap packet (skills, status, warnings) |
| `get_last_updates` | Recent update feed since last session or N hours |
| `check_altevra_skill_version` | Is the installed skill current or outdated? |

## Crate layout

```
crates/
  altevra-core        # Shared types, config primitives
  altevra-db          # Database layer (sqlx, Postgres)
  altevra-skills      # Skill parser, registry, version check
  altevra-bootstrap   # Session bootstrap packet builder
  altevra-hooks       # Hook registry and runner
  altevra-adapters    # Tool adapters (Claude Code, ...)
  altevra-mcp         # MCP JSON-RPC 2.0 server
  altevra-cli         # Binary — all commands
```

## Drift protection

All files written by `altevra connect` carry an `ALTEVRA_MANAGED: true` header.
Re-running `connect` detects manual edits and skips (never overwrites) drifted files.

## Vault structure

```
<project>/
  06-skills/          # Skill markdown files (slug, version, body)
  07-capabilities/    # Capability YAML files
  .altevra/           # Internal state (checksums, registry)
  .claude/            # Claude Code integration (managed)
    altevra-instructions.md
    settings.json
    skills/
      altevra-core.md
```

# Altevra Agent Rules

- Read `ALTEVRA_PRODUCTION_ARCHITECTURE_V5.md` first.
- Build only v5 foundation / nervous system first.
- Rust-first, CLI-first, local-first, MCP-compatible.
- Do not build dashboard/research/connectors yet.
- Do not add external model/API integration yet; local agent attachment/execution is enough for MVP.
- Keep changes scoped, testable, and honest about unimplemented pieces.
- Never commit/push/deploy without Pavle approval.

## Current state (2026-05-27)

**62 tests green. `cargo fmt --check` clean. MCP stdio verified.**

Implemented:
- `altevra init` — vault scaffolding with dry-run + JSON
- `altevra connect --tool claude-code` — real install, drift protection, skill copy
- `altevra skill list/show/check` — registry, `check_version_opt` (not_installed vs outdated)
- `altevra agent bootstrap` — FreshnessCheck + SetupStatus + BootstrapPacket
- `altevra serve --vault` — MCP JSON-RPC 2.0 stdio (3 tools)
- `altevra updates` — update feed stub
- `altevra hook run/list` — hook runner

Not yet implemented (honest list):
- `altevra doctor` — system health diagnostics
- `altevra skill refresh` — re-fetch from source
- `altevra connect --tool codex` — Codex adapter
- Background daemon / file watcher
- Real update feed (non-stub)
- DB layer (altevra-db has schema, no queries yet)

## How to run tests

```bash
cargo test
cargo fmt --check
cargo build
```

## MCP config

Do NOT edit `~/.claude/settings.json` directly.
For MCP mount: use `.mcp.json` at project root (see README.md) or `~/.claude/mcp_config.json`.

# Altevra Full Platform Research Directive

Pavle voice escalation, 2026-05-27 ~03:10:

> Everything must work. The whole platform. Absolutely all functions. Research everything. Split tasks. By morning he wants it to be as close as possible to the best project/platform in the world.

## Interpretation

Do not stop at skeleton. Build the strongest local-first MVP possible overnight while staying safe and testable.

## Required platform pillars

1. **Core Agent OS**
   - events
   - update_feed / last updates
   - change journal
   - agent bootstrap packet
   - context freshness protocol

2. **CLI Platform**
   - `init`
   - `updates`
   - `skill list/check/show`
   - `hook list/run/status`
   - `connect --tool claude-code --project ... --dry-run`
   - `agent bootstrap --json`
   - `doctor`
   - stable JSON output for automation

3. **Skills + Hooks**
   - universal skill schema/parser/version/checksum
   - universal hooks registry
   - session_start/session_end hook runner
   - generated managed headers
   - drift detection placeholder

4. **Adapters**
   - ToolAdapter trait
   - Claude Code adapter skeleton
   - Codex/Cursor/Aider/Antigravity placeholders only if cheap/scalable
   - no hardcoded Claude logic in core

5. **DB + migrations**
   - Postgres migration SQL for all foundation tables
   - repository interfaces or in-memory fallback if DB unavailable
   - testable schema files

6. **MCP skeleton**
   - handlers/tools share same core as CLI
   - `get_agent_bootstrap_packet`
   - `get_last_updates`
   - `check_altevra_skill_version`

7. **Research-backed product quality**
   - Use research/reviewer agents' `.hermes/*.md` reports.
   - Identify best patterns from Agent OS / context engineering / MCP / memory products.
   - Convert into concrete local MVP features.

8. **Testing and scalability**
   - `cargo fmt --check`
   - `cargo check --workspace`
   - `cargo test --workspace`
   - CLI smoke tests where practical
   - clean crate boundaries
   - no hardcoded absolute Pavle paths in library code
   - deterministic output

## Prioritization

If time is limited:

1. Make it compile.
2. Make CLI demo work.
3. Make bootstrap/updates/hooks/skills usable.
4. Add tests for core logic.
5. Add docs/examples.
6. Leave advanced connectors/dashboard as explicit `Not implemented yet` with clean extension points.

## Hard safety boundaries

Allowed: local files, cargo commands, generated docs, local test data.

Not allowed: commit, push, deploy, external API integration, reading/printing secrets, contacting anyone, payments, destructive shell.

## Morning success definition

Pavle can run local commands and see a coherent platform:

```bash
cargo test --workspace
cargo run -p altevra-cli -- init --dry-run
cargo run -p altevra-cli -- updates --json
cargo run -p altevra-cli -- skill list
cargo run -p altevra-cli -- hook list
cargo run -p altevra-cli -- connect --tool claude-code --project altevra --dry-run
cargo run -p altevra-cli -- agent bootstrap --tool claude-code --project altevra --json
```

If any command does not work, document exact blocker and next fix.

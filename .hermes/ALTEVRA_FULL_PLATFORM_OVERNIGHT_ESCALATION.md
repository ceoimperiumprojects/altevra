# Altevra FULL Platform Overnight Escalation

Pavle explicitly escalated: "Ma teraj SVE BRE ima vremena preko noci SVE da napravi sigurnoo bratee. Laku noc sutra hocu da se probudim sa platformom"

Interpretation: push beyond skeleton into a usable local-first MVP platform by morning, while staying safe and scoped.

## Non-negotiable safety boundaries

- Do not commit, push, deploy, publish, contact anyone, buy anything, or perform external side effects.
- Do not read/print secrets.
- Do not add external paid/API model plumbing yet.
- Local file edits, Rust builds/tests, local DB migrations, docs, examples, and local scripts are allowed.

## Goal by morning

A working local Altevra MVP platform, not just architecture docs.

It should be possible to:

1. Run `cargo check --workspace` successfully.
2. Run `cargo test --workspace` successfully or have clearly documented known failing tests.
3. Run a CLI binary locally.
4. Initialize a local Altevra project/vault layout.
5. Create/list updates/events locally.
6. Parse/list/check skills.
7. Run universal hooks locally.
8. Dry-run `connect --tool claude-code --project altevra` and see generated files with managed headers.
9. Call `agent bootstrap --tool claude-code --project altevra --json` and get useful bootstrap JSON with last updates, skill freshness, setup status, warnings.
10. Have MCP skeleton compile and expose the 3 core tool handlers at code level.
11. Have README + examples that explain exactly how Pavle runs it tomorrow.

## Expanded build checklist

### A. Rust workspace + compile

- Finish all 8 crates.
- Make APIs coherent and shared through core types.
- Ensure dependency graph has no cycles.
- `cargo fmt` clean.
- `cargo check --workspace` clean.

### B. Storage layer

If Postgres is not locally available, still implement migrations and provide an in-memory/local-file fallback for CLI demo where practical.

- SQL migrations in `crates/altevra-db/migrations/`.
- DB repository traits + Postgres implementations where feasible.
- File/in-memory repository implementation for local demo/testing if faster.

### C. CLI MVP

Commands should work enough to demo:

```bash
cargo run -p altevra-cli -- init --path .
cargo run -p altevra-cli -- updates --json
cargo run -p altevra-cli -- skill list
cargo run -p altevra-cli -- skill check --all
cargo run -p altevra-cli -- hook list
cargo run -p altevra-cli -- hook run session_start --tool claude-code --project altevra
cargo run -p altevra-cli -- connect --tool claude-code --project altevra --dry-run
cargo run -p altevra-cli -- agent bootstrap --tool claude-code --project altevra --json
```

### D. Vault/project layout

Create generated/sample project layout:

- `06-skills/altevra-core.md`
- `07-capabilities/hooks.yaml`
- `07-capabilities/agent-tools.yaml`
- `15-generated/setup-packs/claude-code/manifest.json`
- `examples/`

### E. Skill system

- Parse YAML frontmatter.
- Extract name/version/description.
- Compute checksum.
- Detect installed/current/outdated/drifted where possible.
- Provide tests.

### F. Updates/events

- Strong types for events/update_feed.
- Event-to-update classifier MVP.
- CLI can show sample/current updates.
- JSON and human output.
- Read state stub okay.

### G. Hooks

- Universal hook definitions.
- Hook runner skeleton.
- session_start/session_end work locally and emit events.
- Hook run JSON output.

### H. Adapter system

- ToolAdapter trait.
- Claude Code adapter with generated files:
  - `CLAUDE.md` snippet or managed instruction file
  - `.claude/skills/altevra-core.md` dry-run path
  - `.claude/settings.json` hook example/dry-run plan if safe
- Managed headers/checksums.
- Drift detection placeholders.

### I. Bootstrap

- Bootstrap packet includes:
  - tool_name
  - project
  - skill_version_current/latest
  - setup_status
  - last_updates
  - active_tasks placeholder
  - goals placeholder
  - warnings
  - recommended_next_action

### J. MCP skeleton

- Compile crate.
- Handlers/functions for:
  - `get_agent_bootstrap_packet`
  - `get_last_updates`
  - `check_altevra_skill_version`
- It can be a skeleton server if full protocol takes too long, but APIs must be testable.

### K. Tests/docs

- Unit tests for parser/checksum/classifier/managed header/bootstrap packet.
- README with "Run tomorrow" section.
- `TODO.md` with honest next steps.

## Execution strategy

Use max effort. Use internal subagents/parallel reasoning if available. If time gets tight, prioritize a compiled CLI demo over perfect architecture.

Do not stop at skeleton if you can build more. Keep going until morning or until blocked by an unavoidable local issue.

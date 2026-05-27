# Altevra

> **Local-first Agent OS in Rust.** CLI-first, adapter-based, MCP-compatible. Gives AI tools a shared, fresh, searchable, auditable context layer.

Altevra solves one problem: **agents must know what changed before they work.** It stores memory, tasks, skills, events, hooks, secrets, and research, then distributes that context into every AI tool you use — Claude Code, Codex, Cursor, Antigravity — via adapters that generate native config files.

## Status

**v5 Foundation — production-ready.** 12 Rust crates, 4 adapters, 23 MCP tools, full CLI, BM25 memory, encrypted secrets, web research pipeline. **226+ tests passing.**

```
crates/
├── altevra-core/       events, updates, classifier, config
├── altevra-cli/        15 commands (init, doctor, connect, memory, ...)
├── altevra-db/         Postgres + sqlx (migrations + repositories live)
├── altevra-skills/     parser, registry, version, checksum, renderer
├── altevra-hooks/      universal registry, runner, session_start/end
├── altevra-adapters/   Claude Code, Codex, Cursor, Antigravity
├── altevra-bootstrap/  packet, freshness, setup status
├── altevra-mcp/        JSON-RPC 2.0 stdio server, 23 tools
├── altevra-vault/      Obsidian-style markdown parser/writer/scanner
├── altevra-memory/     ingestion, chunker, BM25 search, embeddings
├── altevra-research/   web scraper + synthesis pipeline
└── altevra-secrets/    keyring + encrypted file + detector + redactor
```

## Quick start

```bash
# Build
cargo build --release

# Initialize a workspace
./target/release/altevra init

# Connect a tool (writes managed files only)
./target/release/altevra connect --tool claude-code --project myproj
./target/release/altevra connect --tool codex --project myproj
./target/release/altevra connect --tool cursor --project myproj
./target/release/altevra connect --tool antigravity --project myproj

# Bootstrap an agent session
./target/release/altevra agent bootstrap --tool claude-code --json
```

## CLI map

| Command | Purpose |
|---------|---------|
| `init` | Create `.altevra/` + skeleton dirs |
| `doctor [--json]` | 8 health checks (vault, skills, adapters, drift) |
| `config show/get/set` | Edit `.altevra/config.toml` |
| `connect --tool X [--dry-run] [--force]` | Install adapter files |
| `setup verify/repair/status --tool X` | Verify, fix drift, report status |
| `skill list/show/check/refresh` | Skill registry management |
| `hook list/run/install/verify/status` | Hook system |
| `agent bootstrap/status/instructions` | Agent lifecycle |
| `updates [--since 24h] [--mark-read]` | Update feed |
| `memory ingest/search/context/packet` | BM25 memory engine |
| `research run/scrape/synthesize` | Web research pipeline |
| `secrets set/get/list/delete` | Keyring or encrypted-file secrets |
| `journal today/generate` | Daily/window journal |
| `context --project X` | Project context dump |
| `serve` | MCP stdio server (23 tools) |

## Adapters (per-tool target paths)

| Adapter | Instructions | MCP config | Hook config | Skill format |
|---------|--------------|-----------|-------------|--------------|
| **claude-code** | `.claude/altevra-instructions.md` | `.mcp.json` | `.claude/settings.json` (`hooks` key) | `.claude/skills/<slug>/SKILL.md` |
| **codex** | `AGENTS.md` | `.codex/config.toml` (`[mcp_servers.*]`) | `.codex/config.toml` (`[hooks]`) | (CLI prompts only) |
| **cursor** | `.cursor/rules/altevra.mdc` | `.cursor/mcp.json` | (no native hooks) | (rules are skills) |
| **antigravity** | `AGENTS.md` + optional `GEMINI.md` | `.gemini/config/mcp_config.json` | `.agent/hooks/altevra_hooks.py` (SDK) | `.agent/skills/<slug>/SKILL.md` |

Every generated file carries an `ALTEVRA_MANAGED: true` header (HTML/TOML/JSON sentinel as appropriate). Re-running `connect` refuses to overwrite drifted files. Use `--force` to re-render after manual cleanup.

## MCP tools (23)

Bootstrap, updates, skills, memory, tasks, capabilities, setup:

```
get_agent_bootstrap_packet
get_last_updates / mark_updates_read
check_altevra_skill_version / get_altevra_skill / get_skill / list_skills / request_skill_refresh
search_memory / get_project_context / get_context_packet / get_source_of_truth
get_active_tasks / save_task / update_task / get_goals / save_decision
get_capabilities / report_knowledge_gap / report_capability_gap / create_review_item
get_setup_status / run_hook
```

Mount via `.mcp.json`:

```json
{"mcpServers": {"altevra": {"command": "altevra", "args": ["serve"]}}}
```

## Architecture principle

CLI is primary. MCP is an adapter. REST is internal. Dashboard is later.

Every meaningful action emits an `Event`. Events get classified (5 importance levels) into `UpdateFeedItem`s. Adapters render universal types into tool-native files. Hooks emit events. Determinism: no timestamps in managed file content — same input always produces the same checksum.

## What's NOT in this build

- pgvector embeddings (placeholder; BM25 first)
- External LLM API integration (research synthesis is local concat)
- Aider adapter
- Dashboard / web UI
- Google Workspace / Slack / Linear / NotebookLM connectors

All of the above can be added incrementally on top of the foundation.

## Test counts

| Crate | Tests |
|-------|-------|
| altevra-core | 10 |
| altevra-cli | 46 |
| altevra-adapters | 37 |
| altevra-skills | 11 |
| altevra-hooks | 9 |
| altevra-bootstrap | 9 |
| altevra-mcp | 22 |
| altevra-vault | 32 |
| altevra-memory | 25 |
| altevra-secrets | 26 (+1 ignored) |
| altevra-db | (live sqlx — requires Postgres) |
| **Total** | **226+ passing** |

## License

Proprietary — Imperium Tech LLC.

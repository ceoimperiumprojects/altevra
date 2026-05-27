<div align="center">

```
    █████╗ ██╗  ████████╗███████╗██╗   ██╗██████╗  █████╗
   ██╔══██╗██║  ╚══██╔══╝██╔════╝██║   ██║██╔══██╗██╔══██╗
   ███████║██║     ██║   █████╗  ██║   ██║██████╔╝███████║
   ██╔══██║██║     ██║   ██╔══╝  ╚██╗ ██╔╝██╔══██╗██╔══██║
   ██║  ██║███████╗██║   ███████╗ ╚████╔╝ ██║  ██║██║  ██║
   ╚═╝  ╚═╝╚══════╝╚═╝   ╚══════╝  ╚═══╝  ╚═╝  ╚═╝╚═╝  ╚═╝
```

### **The omniscient brain layer for your AI tools.**

*Local-first. Source-available. Built in Rust.*

[![Status](https://img.shields.io/badge/status-v0.3_alpha-DC2626?style=flat-square)]()
[![Tests](https://img.shields.io/badge/tests-434_passing-DC2626?style=flat-square)]()
[![Rust](https://img.shields.io/badge/rust-1.75+-DC2626?style=flat-square&logo=rust)]()
[![License](https://img.shields.io/badge/license-PolyForm_Strict-7F1D1D?style=flat-square)](./LICENSE)
[![Commercial](https://img.shields.io/badge/commercial-contact_required-7F1D1D?style=flat-square)](#commercial-licensing)

</div>

---

## What is Altevra?

Altevra is the **shared memory and brain** behind every AI tool you use — Claude Code, Codex, Cursor, Antigravity, Hermes, anything that speaks MCP.

While your AI tools forget everything between sessions, Altevra **remembers**:

- 🧠 **Every tool call** across every AI session, recorded locally
- 🔴 **Every secret** auto-captured to the OS keyring *before* it gets redacted from chat
- 📚 **Every file change** indexed, embedded, and searchable across all your projects
- 🛰️ **24h autonomous brain** that researches, watches RSS feeds, scrapes GitHub Trending, and writes you a daily *leverage brief* per project
- 🔗 **One source of truth** — your `~/.altevra/` directory, your laptop, your control

> **No cloud. No telemetry. No data leaves your machine unless you tell it to.**

---

## Why source-available, not open-source?

Altevra is **PolyForm Strict licensed** — the source is public, you can read it, study it, fork it for **non-commercial purposes**. But it is **not** open-source in the OSI sense.

**Why:**
- Altevra represents thousands of hours of solo work by [Pavle Anđelković](https://www.linkedin.com/in/pavle-andjelković-1614b1373).
- If your company wants to use Altevra commercially — host it as a service, bundle it into a product, or deploy it inside a for-profit organization — [contact us](#commercial-licensing).
- Personal use, hobby projects, academic research, and non-commercial experimentation are **always free**.

See [LICENSE](./LICENSE) for the full PolyForm Strict 1.0.0 terms.

---

## Quick start

```bash
# Build from source (binary releases coming in v0.4)
git clone https://github.com/ceoimperiumprojects/Altevra.git
cd Altevra
cargo build --release

# Initialize Altevra in any project
./target/release/altevra init

# Connect your AI tools (auto-detects what's installed)
./target/release/altevra connect --tool claude-code
./target/release/altevra connect --tool codex
./target/release/altevra connect --tool cursor

# Start the omniscient brain (autonomous research + indexing)
./target/release/altevra brain start

# Generate today's leverage brief
./target/release/altevra research leverage
```

---

## The Brain — what runs in the background

When you start `altevra brain`, ten autonomous jobs schedule themselves on a tokio runtime:

| Job | Period | What it does |
|-----|--------|--------------|
| `event_classifier` | 1 min | Tags every event in your event log |
| `observer_scan` | 5 min | Detects behavioral patterns across sessions |
| `vault_indexer` | 15 min | Catches file changes the watcher missed |
| `embedder_worker` | 5 min | Drains the pending-embeddings queue → Gemini API |
| `insight_synthesizer` | 1 h | "What's interesting in the last hour" (LLM) |
| `research_fetcher` | 2 h | Pulls all configured RSS/Atom feeds |
| `feed_discovery` | 1 h | Auto-discovers new RSS feeds from pages you visit |
| `github_trending_fetch` | 4 h | Scrapes GitHub Trending (Rust, TS, Python) |
| `project_research_sweep` | 24 h | Per-project web search (DDG/Brave/Exa) |
| `daily_summary` | 23:00 | Writes a full daily Obsidian brief |

Every job is independently disable-able. Every job logs to SQLite. Every job is rate-limited under Gemini's free tier.

---

## Per-project research agents

Altevra reads your projects from `~/.imperium/identity/projects.yaml` (or any YAML you point it at) and gives each project its own research agent with a priority-driven daily budget:

```
[P0] revesta              budget: 10 queries/day
[P1] tunia                budget: 5  queries/day
[P1] meta-sales-bot       budget: 5  queries/day
[P2] cograder             budget: 3  queries/day
[P3] hyper-pipeline       budget: 3  queries/day
```

Each agent runs its keywords through DuckDuckGo (free) or Brave/Exa (if you provide API keys), dedupes against history, scores relevance, and pulls top-N hits into your daily leverage brief.

---

## MCP Server — 32 tools

`altevra serve` exposes Altevra as an MCP server over stdio. Any AI tool that speaks Model Context Protocol can call:

```
get_agent_bootstrap_packet       discover_feed
get_last_updates                 github_trending
search_memory                    web_search
get_project_context              project_research
get_context_packet               replay_session
get_active_tasks                 search_turns
save_task / update_task          file_history
save_decision                    get_observer_insights
... and 16 more
```

---

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│  Your AI tools (Claude Code, Codex, Cursor, Antigravity)    │
└────────────┬─────────────────────────────────┬──────────────┘
             │ hooks (40 events)               │ MCP
             ▼                                 ▼
┌─────────────────────────────────────────────────────────────┐
│                       ALTEVRA CORE                          │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌─────────────┐  │
│  │ Recorder │  │ Watcher  │  │  Brain   │  │  Research   │  │
│  │ sessions │  │  vault   │  │ (10 jobs)│  │ (RSS+search)│  │
│  │  turns   │  │ embedder │  │          │  │             │  │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘  └──────┬──────┘  │
│       └─────────────┴─────────────┴────────────────┘        │
│                          │                                  │
│                  SQLite + keyring                           │
│                  (~/.altevra/)                              │
└─────────────────────────────────────────────────────────────┘
                          │
                          ▼
              ┌─────────────────────┐
              │   Obsidian vault    │
              │   (human-visible)   │
              └─────────────────────┘
```

15 Rust crates. Single binary. SQLite-only persistence. Zero network egress except for RSS/HTTP fetching and Gemini embeddings (optional).

---

## Status

**v0.3 alpha** — feature-complete recorder, watcher, embedder, brain, research v2, MCP server, and adapter fan-out. Currently running on the author's daily-driver laptop.

- ✅ **434 tests passing**, 0 failing, 1 ignored
- ✅ 4 AI tool adapters wired (Claude Code, Codex, Cursor, Antigravity)
- ✅ 32 MCP tools live
- ✅ 16 SQLite migrations
- ✅ Auto-capture secrets before redaction
- ⏳ v0.3.8 "Analyze Everything" — historical session import (in progress)
- ⏳ v0.4 — binary releases, install script, install via cargo / homebrew

---

## Commercial licensing

The PolyForm Strict license **does not** permit commercial use. For:

- **SaaS / hosted deployment**
- **Bundling into a commercial product**
- **Internal use at a for-profit organization**
- **Consulting / professional services using Altevra**
- **Removing or modifying license notices**

…contact us for a commercial license:

📧 **ceoimperiumprojects@gmail.com**

---

## Contributing

Until v1.0 the project is single-maintainer by design. PRs are appreciated but not actively solicited — open an issue first if you'd like to propose something significant.

Bug reports, documentation fixes, and small enhancements are always welcome.

---

## Credits

Built by **[Pavle Anđelković](https://github.com/ceoimperiumprojects)**, with engineering assistance from **Claude Opus 4.7** and **Hermes**.

> *"Ne pravim alat. Pravim mašinu koja pravi proizvode."*

---

<div align="center">

**[★ Star on GitHub](https://github.com/ceoimperiumprojects/Altevra)**  •  **[Report a bug](https://github.com/ceoimperiumprojects/Altevra/issues)**  •  **[Commercial license](#commercial-licensing)**

Copyright © 2026 Pavle Anđelković. All rights reserved.

</div>

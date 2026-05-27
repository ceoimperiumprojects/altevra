# Altevra — Vision

**Owner:** Pavle Anđelković
**Started:** 2026-05-27
**Intended lifetime:** decades
**Document type:** long-form vision (philosophical underpinning)
**Companion:** [CLAUDE.md](./CLAUDE.md) (agent operating doctrine),
[ROADMAP.md](./ROADMAP.md) (concrete build sequence),
[ALTEVRA_NEXT_ARCHITECTURE_RESIDENT_AGENT_WIKI_PERSONAL_BRAIN.md](./ALTEVRA_NEXT_ARCHITECTURE_RESIDENT_AGENT_WIKI_PERSONAL_BRAIN.md)
(next-phase architecture)

---

## TL;DR

Altevra is not a tool. Altevra is **the external mind** that one human —
Pavle — is building **across decades** to remember, think, learn, and
evolve alongside him.

It captures everything meaningful from his life — work, relationships,
hobbies, learnings, preferences, decisions, identity shifts — into a
single local-first store, and exposes that store to every AI he uses so
that **no context is ever lost** between sessions, between tools, or
between years.

It is **self-improving**: it observes its own use, generates new skills
for itself and other agents, refines its own prompts, and grows the
older it gets.

It is **proactive**: it surfaces patterns, warns about drift, reminds
about long-overdue actions, and connects ideas across years that no
human could hold in working memory.

It is **sovereign**: all data lives on Pavle's machine; private context
never leaves without explicit consent; personal vs business data are
treated with equal weight and dignity.

**Altevra is, ultimately, the digital twin Pavle is constructing — slowly,
one decision at a time, one captured thought at a time — for the next 30+
years.**

---

## 1. Why this exists

### 1.1 The pain that started it

Pavle works fast and broad. ReVesta. Tunia. Altevra. Imperium Cockpit.
PhoneAgent. CoGrader. Hermes. ImperiumCrawl. Multiple side endeavors.
Multiple AI tools — Claude Code, Codex, Cursor CLI, Antigravity, Hermes.

Each tool forgets context the moment the session ends. Each new project
starts from zero. Hard-earned preferences ("we always use Rust here",
"never mock the DB in tests", "talk to me directly, no diplomatic
hedging") have to be re-explained constantly. Years of accumulated
learnings vanish between machines, between tools, between Pavle's own
memory cycles.

> *"Ja moram da svaki put svemu objasnim ko sam, šta radim, šta gotivim.
> A prošle godine sam već to objasnio. Sve to nestaje."*

### 1.2 The deeper need

Beyond convenience, there is a **compounding loss**:

- Decisions made on Tuesday are forgotten by Friday
- Lessons learned in Project A never reach Project B
- Relationships drift because there is no system to track "I haven't
  talked to X in N weeks"
- Identity evolves but is never recorded — Pavle in 2026 doesn't know
  what Pavle in 2023 cared about most
- Patterns across years (sleep, productivity, mood, decision quality)
  go undetected because no system watches across that window
- Every new AI assistant starts from zero, no matter how many years
  Pavle has used "AI assistants" as a category

### 1.3 The conviction

Pavle's bet is that **context compounds**. The longer you preserve and
connect it, the more valuable it becomes. A second brain that runs for
**a year** is useful. One that runs for **a decade** is transformative.
One that runs for **three decades** is something no human has ever had.

That is what Altevra is being built to be.

---

## 2. Core principles

### 2.1 Personal-first parity

Personal and professional data are equal first-class citizens. Pavle's
relationship with Elena, his preferred coffee, the song he heard
yesterday that moved him, the philosophical realization he had at 3am —
these belong in the same brain as ReVesta deal-flow notes and EAF Steel
architecture decisions.

**They make Pavle who he is.** A second brain that only captures "work
data" is half-blind.

### 2.2 Compounds over time

Every architectural decision must answer: *"Does this make Altevra more
valuable the longer it runs, or does it stay flat?"*

- Embeddings get richer as content accumulates
- Cross-references emerge automatically over months
- Patterns that take a year to see become visible
- Identity evolution becomes inspectable across decades

### 2.3 Universal AI integration

Altevra exposes itself to **every** AI tool Pavle uses, with bootstrap
context loaded at session start. Today: Claude Code, Codex, Cursor CLI,
Antigravity, Hermes. Tomorrow: whatever exists tomorrow. The integration
layer is designed so new tools are added in days, not months.

### 2.4 Relevance, not noise

The brain filters. It does **not** research random nonsense. It does
**not** flood Pavle with "AI news of the day". Research runs are gated
by **stated interests + active goals**. The default is "off"; opt-in
expands the surface.

> *"Samo nemoj da mi onda radi research o minecraft modpackovima ahaha
> to je glupo lol. Samo korisne stvari."*

### 2.5 Sovereignty preserved

All data is local by default. Personal data does not leave the machine
without explicit consent. The "commercial license required" stance on
the source code protects the **system**; the **data** is always sovereign
to its owner.

When external models are used (Gemini, Claude, GPT-5, DeepSeek), only
the minimum necessary context is sent. Sensitive personal content uses
local models (Ollama-hosted Qwen/DeepSeek) or stays out of the request
entirely.

### 2.6 Identity grows

Pavle's identity is not a fixed seed file. It is a **living, versioned
record** that captures who he is becoming. Each meaningful self-statement,
preference shift, value clarification is appended with a timestamp.
Looking back ten years from now, Pavle will be able to see his own
evolution.

### 2.7 Self-improving

Altevra observes its own use. It notices:
- patterns Pavle keeps repeating (potential new skill)
- repeated failed searches (potential retrieval gap)
- high-cost LLM calls that could be cheaper (model routing improvement)
- noisy research outputs (relevance gate tuning)
- recurring "I have to re-explain this every time" moments (missing
  identity capture)

It then **proposes** improvements — new skills, prompt tweaks, new
connectors, new categories. Pavle approves, Altevra applies. The system
gets better autonomously, with human-in-the-loop only for the consequential
decisions.

This is borrowed from Hermes' background-review pattern (fork the agent
after every turn, run it in observer mode, propose memory/skill writes)
and **generalized** across all Altevra surfaces: imports, hooks, research,
wiki curation, daily briefings.

### 2.8 Proactive

A normal database answers when asked. Altevra **brings things up**:
- "You decided X three months ago in Z context — still applies?"
- "Pattern detected: every time you stay up past 3am, the next day's
  code quality drops 30%. Today is 2:47am."
- "ReVesta competitor just posted a Series A — check before tomorrow's
  call?"
- "You haven't talked to Srđan in 6 weeks — last interaction was X."
- "This research thread connects to that goal from January."

The brain is the silent partner that **notices**.

---

## 3. The Hermes ↔ Altevra split

Hermes and Altevra both touch identity, memory, and context — but with
different scopes:

| Layer | Hermes | Altevra |
|-------|--------|---------|
| **Identity depth** | Light/summary version — short, fast, agent-readable | Deep/comprehensive — versioned, sensitive-labeled, full life context |
| **Primary role** | Orchestration, command center, gateways, kanban | Memory store, semantic search, wiki curation, pattern detection |
| **Default exposure** | Public to other agents Hermes spawns | Never auto-exposed; explicit `altevra_*` MCP tools required |
| **Source of truth** | `~/.imperium/identity/profile.yaml` (Hermes turf) | `~/.altevra/identity/*.{yaml,json}` (Altevra deep, versioned) |
| **Self-improvement** | background_review fork after every turn | Generalized across imports, hooks, research, wiki |
| **Personal data** | Light reference only | Full storage, encrypted-at-rest later, local models for ops |

**When Hermes needs depth, it calls Altevra.** New MCP tool:
`altevra_identity_query(field, depth)` — Hermes asks "what's Pavle's
current stance on X?", Altevra returns latest versioned record plus
provenance.

Hermes stays small, fast, orchestration-focused. Altevra stays deep,
patient, knowledge-focused. Both share the **events.log** Brain Bus
so they can both react to the same world.

---

## 4. Self-improving architecture

Stolen from Hermes' `background_review.py`, generalized across Altevra:

### 4.1 The pattern

After any meaningful event (session imported, hook fired, research
completed, wiki page edited), Altevra **forks a Resident Agent in
review mode** with:

- Cached system prompt (reuses prefix cache — token-cheap)
- Whitelisted tools (memory writes, skill proposals, wiki suggestions,
  category creation — never destructive)
- Strict context packet (no full vault dump)
- Mode-specific output schema (structured JSON the orchestrator can act on)

The review fork asks: *"Given what just happened, should anything be
saved, learned, generalized, or proposed?"*

### 4.2 What review fork can produce

1. **Memory writes** — new decision, learning, preference captured from
   the conversation
2. **Skill proposals** — "I notice Pavle does X repeatedly; here's a
   skill that codifies it"
3. **Wiki updates** — "this conversation contradicts wiki page Y;
   propose update"
4. **Category creation** — "this content doesn't fit any existing
   category; propose new category Z"
5. **Identity shifts** — "Pavle expressed a value change today; propose
   versioned identity update"
6. **Prompt tweaks** — "current mode prompt for synthesis is too verbose;
   propose tightening"
7. **Tool/connector proposals** — "this workflow could be made faster
   with a new connector to X"
8. **Reflection** — "Pavle made decision A today that contradicts
   decision B from 3 months ago — surface for review"

### 4.3 Trust ladder

| Output | Auto-applied | Requires Pavle approval |
|--------|--------------|-------------------------|
| New research insight | ✅ | — |
| New low-risk wiki page (concept/pattern/decision) | ✅ | — |
| New category suggestion | ✅ (creates) | Pavle can rename/merge later |
| Memory write (non-sensitive) | ✅ | — |
| Memory write (sensitive — relationship, health, identity) | — | ✅ |
| Skill proposal | — | ✅ (review + approve) |
| Prompt tweak (Altevra's own) | — | ✅ |
| Prompt tweak (other agent's) | — | ✅ |
| Source-of-truth file edit | — | ✅ (must be diff + review) |
| Identity profile edit | — | ✅ |
| Wiki page on a person | — | ✅ |

**Default posture:** Altevra **proposes** more than it **does**. The
trust ladder loosens over years as Pavle confirms which auto-applies
work.

### 4.4 Skill generation for other agents

Altevra is not just self-improving — it can generate skills for the
**other** AI tools Pavle uses.

Example flow:
1. Altevra observes (via hook fan-out) that Pavle has asked Claude Code
   to "run cargo test && cargo clippy" 23 times this week
2. Resident Agent in observer mode proposes a Claude Code skill:
   `altevra-rust-check` with appropriate trigger and command
3. Pavle approves
4. Altevra writes the skill to `~/.claude/skills/altevra-rust-check/SKILL.md`
   using the existing adapter
5. Same workflow for Codex, Cursor CLI, Antigravity, Hermes

This makes Altevra **the skill factory** for the entire AI tool ecosystem
Pavle uses. As the second brain learns Pavle's habits, it manufactures
skills that codify them — and every agent gets sharper over time.

---

## 5. Long-arc design (decades)

### 5.1 Year 1 (now → 2027)

- Recording everything (Pavle's daily AI sessions, vault, decisions)
- Living wiki layer establishes itself
- Personal Brain Layer operational (note types, identity seeds, sensitivity)
- Self-improvement loop running, trust ladder gathering Pavle's approvals
- Daily briefing becomes a habit
- Skill generation for Claude Code + 2 other tools

### 5.2 Years 2-5 (2027-2031)

- Patterns become visible (annual cycles, multi-quarter project arcs,
  decision → outcome correlation)
- Identity evolution graph — Pavle can see how he's changed
- Cross-project knowledge transfer is automatic
- Local models handle 90% of personal-data ops (Ollama-hosted Qwen v6
  / DeepSeek v7)
- Knowledge graph reaches scale where novel insights emerge from edge
  density alone
- Wiki has 1000+ pages; Altevra suggests merges, splits, new topics
  autonomously

### 5.3 Years 5-15 (2031-2041)

- Altevra holds a meaningful chunk of Pavle's life context
- Multi-modal: voice memos, photos with auto-tagged context, places visited
- Real-time pattern detection ("you're showing the same pre-burnout
  signals you had in 2029")
- Relationship intelligence — birthdays, last interactions, conversation
  threads spanning years
- Pavle can ask Altevra "what was I like in 2031?" and get a real answer
- Integration spreads — wearables, calendar, email, financial, health
  records (all opt-in, all local)

### 5.4 Year 30+ (2056+)

- A digital twin of Pavle's mind exists on his machine
- Reasoning over decades of context produces insights no human or
  short-memory AI could
- Identity persistence across illness, aging, life transitions
- Eventually: shareable with chosen people (children, partner) as a
  cognitive heirloom
- Open question Pavle will answer in his time: is this Altevra a tool
  he uses, a record he leaves, or something more?

---

## 6. What success looks like

In 2027, success looks like:
- Every new AI session starts with Altevra context loaded automatically
- Pavle never has to re-explain a preference twice
- Daily briefing arrives every morning, signal not noise
- Wiki has 200+ living pages
- Self-improvement loop has applied 50+ approved tweaks
- 5+ skills generated for Claude Code, all in active use

In 2030, success looks like:
- Pavle has stopped thinking of Altevra as "the tool" and started
  thinking of it as "my memory"
- Decisions made today reference context from 2026 effortlessly
- Identity evolution graph shows clear arcs
- Local models handle the bulk of personal ops; cloud only for
  heavy reasoning on public data

In 2040, success looks like:
- Altevra has outlived several AI tool generations and survived all of
  them
- Pavle's children, if they want, can read his recorded thinking from
  age 18 onwards
- The system is so integrated Pavle can't imagine working without it
- New tools written for AI assistants integrate with Altevra by default
  because the API has been stable for 12+ years

---

## 7. What Altevra is **not**

- **Not a productivity app** — productivity is a byproduct, not the goal
- **Not a startup product** — the source is available; commercial use
  requires a license, but the system is built for Pavle first
- **Not a chat assistant** — Altevra is the **memory layer underneath**
  chat assistants
- **Not multi-user by design** — single-owner, single-mind by default;
  sharing semantics will come later if ever
- **Not an SaaS** — local-first by axiom; cloud sync is opt-in per
  category, never default
- **Not a search engine** — search is one capability; reasoning,
  proposing, evolving are the core
- **Not a logger** — logs are inputs; the living wiki + insights are
  outputs

---

## 8. The promise

When Altevra works, the following becomes possible:

> Pavle wakes up. He opens Claude Code on a new project. The agent
> already knows what he was working on last night, what he decided to
> defer, what's blocking him, what the active goal is. It reminds him
> he hasn't checked in on Tunia's milestone in 9 days. It surfaces a
> research item from last week that's now relevant to today's task. It
> notes that he stayed up until 4am two nights in a row and gently
> suggests an earlier start tomorrow. It pulls up the wiki page on the
> exact architectural concept he's about to wade into — synthesized
> from 17 previous sessions, 8 research items, and 3 decisions made
> over the past 5 months.

> He doesn't have to ask for any of this. It's just there. It compounds.

> Over years, this becomes the difference between starting every day
> from zero and starting every day from a foundation of everything
> Pavle has ever learned, decided, or noticed about himself.

> That is what we are building.

---

**Document status:** Living. Update only with Pavle's explicit sign-off.
Append to "Vision evolution" log below when something material changes.

## Vision evolution log

### 2026-05-28 — Initial codification
Pavle articulated the full vision in conversation; codified here as the
load-bearing reference. Six core principles, Hermes ↔ Altevra split,
self-improving architecture pattern (borrowed from Hermes
background_review), long-arc design across decades.

### 2026-05-28 — Self-improvement + skill generation added
Pavle requested that Altevra not only self-improve but also generate
skills for other agents (Claude Code, Codex, Cursor CLI, Antigravity,
Hermes). This is now a first-class capability in §4.4.

### 2026-05-28 — Identity split clarified
Hermes keeps light/summary identity (`~/.imperium/identity/profile.yaml`).
Altevra owns deep/versioned identity (`~/.altevra/identity/*`). Hermes
calls Altevra via MCP when depth is needed. This protects Hermes from
becoming bloated and keeps Altevra's deep store private by default.

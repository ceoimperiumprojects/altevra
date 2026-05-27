# Altevra — Vision & Operating Doctrine

**Owner:** Pavle Anđelković
**Started:** 2026-05-27
**Intended lifetime:** decades

> This is not a productivity tool. This is not a startup project.
> **Altevra is Pavle's external mind** — a personal second brain that
> grows with him for decades, captures every meaningful thought,
> preference, decision, relationship, learning, and goal across his
> entire life, and gives that context back to every AI he uses.

---

## 1. Why Altevra exists

Pavle works fast. He spans many projects, many tools, many domains:
ReVesta, Imperium Cockpit, PhoneAgent, Tunia, CoGrader, Hermes,
Altevra, ImperiumCrawl, plus 5+ side endeavors. He uses Claude Code,
Codex, Cursor CLI, Antigravity, Hermes — each forgetting context the
moment a session ends. He keeps notes in Obsidian, decisions in his
head, preferences nowhere persistent. Knowledge that *should*
compound across years gets lost between sessions.

**The pain:** "I have to re-explain myself to every AI, every time.
Past learnings vanish. Context is rebuilt from zero. Years of accumulated
preferences and decisions don't carry forward."

**Altevra solves this by becoming the single source of truth that
*every* AI tool talks to**, and that holds **every** piece of context
Pavle has ever produced — work, personal, business, life, hobbies,
relationships, learnings, preferences, goals.

---

## 2. Core Vision — verbatim from Pavle (2026-05-28)

> *"Trebalo bi da imamo i personal stvari. Personal stvari, personal
> references, nešto iz života mog, ne samo vezano za startup i biznis,
> pošto ja prečam u svemu. ... Fora je da može u bazu da se trpa baš sve
> sve i biznis i život i zabava i sve bukvalno. To je general baza svega
> koja povezuje sve stvari i iz života što je generalno jako jako korisno
> i treba tako da bude."*

> *"Ovo radimo da ga koristim long term. Godinama bukvalno možda čak i
> decenijama. To hoću da postignem sa ovim. Sve da može da stane u ovu
> bazu — ceo moj život, sve moje, brate, sve sve."*

> *"Cilj je da može da se koristi za sve. Da mogu da koristim bukvalno za
> sve životne stvari. Da se sve pamti, da misli baza, da misli, da
> obaveštava, da traži research, da skuplja vesti. Da jednostavno
> long-term leverage."*

> *"Bukvalno pravim kroz godine ću napraviti digitalnu verziju sebe.
> Tako da ovo je baš drugi mozak sistem."*

> *"Da AI koji razmišlja bude mu lako, da embedding može da traži lako.
> Da se knowledge i znanje i preferencije i sve o meni kompounduje kroz
> vreme. ... Ovo je stvarno jedan kontekst inženjering na jednom jako
> visokom nivou."*

---

## 3. What Altevra MUST be

### 3.1 Personal AND professional — equally first-class

The database holds, with the same dignity:

| Category | Examples |
|----------|----------|
| **Business** | Decisions, customer notes, GTM playbooks, deal history, OKRs |
| **Personal** | Relationships (Elena, family), health, fitness, mood patterns |
| **Hobby / interests** | Music tastes, books read, films, places visited, ideas |
| **Learning** | New concepts mastered, mistakes made, post-mortems |
| **Preferences** | Coding style, communication style, food, travel, what excites Pavle |
| **Goals** | Daily, weekly, quarterly, decade — across life domains |
| **Memory** | People met (with context), conversations had, lessons learned |
| **References** | Books, papers, articles, repos, videos worth remembering |
| **Schedule / patterns** | When Pavle works best, sleep, energy cycles |
| **Identity** | Who Pavle is becoming — character, values, evolution over years |

**Not allowed:** treating "work data" as more legitimate than "personal data."
Pavle's relationship with Elena, his preferred coffee, the song he heard yesterday
that moved him — these all belong in the same brain. They make him *him*.

### 3.2 Auto-categorization

When new data lands, Altevra should:
1. Detect category from content (LLM-assisted classification)
2. If existing category fits → tag it
3. If no existing category fits → **propose and create a new one** automatically
4. Surface the new category to Pavle on his daily digest so he can rename/merge

Categories are a *living taxonomy*, not a fixed schema.

### 3.3 Smart research filter — relevance gate

> *"Samo nemoj da mi onda radi research o minecraft mode pack-ovima ahahah
> to je glupo lol. Samo korisne stvari."*

The brain's research jobs must run **a relevance gate** before pulling external
content. Rules:
- Auto-research runs only on **active goals** + **stated interests**, not every
  passing keyword
- "Useful" is defined as: contributes to a goal, deepens an interest Pavle has
  explicitly opted into, or warns about a risk to something Pavle cares about
- Trivia / entertainment-only / "ego scrolling" content is filtered out by
  default; Pavle can opt-in per category (e.g. "do research music releases
  by Nils Frahm" — yes; "Minecraft modpacks" — no)

The filter is a **stated preferences** layer above the research engine —
not a generic "rank by score" heuristic.

### 3.4 Compounding knowledge — design for decades

Every design decision must answer: **"Does this make Altevra more valuable
the longer it runs, or does it stay flat?"**

Concretely:
- Embeddings get richer as content accumulates → semantic search gets *better*
  over time, not worse
- Patterns surface that only become visible after months/years (sleep + mood
  correlation, productivity cycles, etc.)
- Cross-project connections emerge automatically (e.g. "the ReVesta GTM
  lesson from Mar 2026 applies to Tunia GTM in Sept 2027")
- Relationships maintained across thousands of interactions
- Preferences refined as Pavle changes — version history preserved

### 3.5 Universal AI tool integration

Altevra exposes itself to **every** AI tool Pavle uses, with **bootstrap
context loaded at session start**:

- Claude Code (MCP server `altevra serve` + hooks) ✅
- Codex CLI (config.toml hooks) ✅
- Cursor CLI (`.cursor/hooks.json` + ai-tracking SQLite import) — partial,
  needs CLI-specific adapter
- Antigravity (gemini-cli history) ✅
- Hermes (session_*.json import) ✅
- Future: any new tool that arrives gets an adapter within days, not months

Every session begins with Altevra delivering: relevant past decisions,
active goals, recent learnings, current preferences. Every session ends
with Altevra capturing what changed.

### 3.6 The brain is **proactive**, not just queryable

A normal database answers when asked. Altevra **brings things up**:
- "You decided X three months ago in Z context — still applies?"
- "Pattern detected: every time you stay up past 3am, the next day's code
  quality drops 30%. Today is 2:47am."
- "ReVesta competitor just posted a Series A — check before tomorrow's call?"
- "You haven't talked to Srđan in 6 weeks — last interaction was X."
- "This research thread connects to that goal from January."

The brain is the silent partner that *notices*.

---

## 4. Architectural commitments that follow from the vision

### 4.1 Storage is **one** unified store, not silos

- Personal + business + everything → same SQLite database, same embedding space
- Different *categories*, same physical store
- Cross-category queries are first-class ("show me all decisions related to ReVesta
  AND that involve Elena's input AND that I made when I was tired")

### 4.2 Embeddings everywhere

Every meaningful unit gets embedded:
- Every turn in every session
- Every decision, learning, preference, goal
- Every Obsidian doc, every research item, every captured thought
- Embedding dim consistent across types (cross-type semantic search)

### 4.3 Provenance + temporal context

Every record carries:
- **When** it was captured
- **Where** it came from (which session/tool/source)
- **Who** said it (Pavle vs AI vs imported)
- **Confidence** (Pavle's direct statement vs AI-inferred)
- **Linked records** (graph edges — this decision relates to that goal)

### 4.4 Privacy + sovereignty

Altevra is **local-first by axiom**. Cloud sync is opt-in per category,
never default. Personal data never leaves Pavle's machine without explicit
authorization. The "commercial license required" stance on the source code
protects the *system*; the data is always sovereign to its owner.

### 4.5 Identity persistence

Pavle's identity (`~/.imperium/identity/profile.yaml`) is the seed. Altevra
*grows* that identity over time — capturing micro-preferences and macro-shifts,
keeping a versioned history so Pavle can see how he's evolved.

---

## 5. What this means for current development

| Sub-version | New mandate from vision |
|-------------|-------------------------|
| **v0.3.9 Multi-Provider LLM** | LLM not just for chat — also for: auto-categorization, relevance filtering, identity classification, pattern naming, summary distillation. Multi-provider so Chinese/local models can do private personal classification without sending Pavle's life to US clouds. |
| **v0.3.10 Onboarding** | First-run wizard MUST ask: "What life domains do you want Altevra to track?" — defaults that include personal, not just code |
| **v0.4 Personal Brain Layer (NEW)** | Dedicated module for: note types (decision, learning, preference, person, place, idea), auto-categorization engine, relevance filter, proactive notifier. **Without this Altevra is just a recorder.** |
| **v0.5 Cursor CLI Adapter (refined)** | Import from `~/.cursor/ai-tracking/ai-code-tracking.db` (50K+ AI code hashes), `~/.cursor/plans/*.plan.md`, `~/.cursor/projects/*/repo.json`. Trim down the existing VS Code chatSessions parser since real Cursor CLI doesn't store JSONL — it stores SQLite. |
| **v0.6 Knowledge Graph** | Edges between entities (people, projects, decisions, goals) so Altevra can answer "what's connected to what" — the cross-pollination engine Pavle needs |
| **v0.7 Pattern Detection** | Detect cycles (sleep/productivity), relationships (decisions → outcomes), correlations across categories. Surface as morning briefs. |
| **v0.8 Active Briefings** | Proactive notifications: "Re-check this decision," "Pattern broken," "Long-overdue reach-out" |

These are not "nice to haves" — they are the **central feature set** that
distinguishes Altevra from a logger.

---

## 6. Operating doctrine for any agent (Claude, Hermes, future)

When working in this repo, **read this file first**. Every architectural
decision must be checked against the vision:

1. **Personal-first parity** — does this feature treat personal data with
   equal weight to business data?
2. **Compounds over time** — does this make Altevra better the longer it
   runs?
3. **Universal integration** — does this work across all AI tools, or only
   one?
4. **Relevance, not noise** — does this filter add signal, or generate
   bullshit research?
5. **Sovereignty preserved** — does this keep Pavle's data local and his?
6. **Identity grows** — does this help Pavle build a digital version of
   himself over decades?

**If the answer is "not really" for more than one of these — push back
or redesign before building.**

When in doubt, **ask Pavle**, and **save his answer to this file** under
"Decisions" so the next agent has it.

---

## 7. Decisions log

### 2026-05-28 — Vision codified
Pavle stated the long-term vision for Altevra: second brain, decades-long
artifact, personal + business equally, auto-categorization, smart research
filter, integrated into every tool, compounding knowledge. This document
captures that and becomes the load-bearing reference for all future work.

### 2026-05-28 — Cursor CLI storage understanding
Cursor CLI (the standalone `cursor` agent, not the VS Code Cursor extension)
stores data in `~/.cursor/ai-tracking/ai-code-tracking.db` SQLite + plans in
`~/.cursor/plans/*.plan.md`, NOT in `chatSessions/*.jsonl` files. The
current v0.3.8 Cursor parser only handles the VS Code extension format
(which generates empty files on this system since Pavle uses the CLI).
v0.5 will add a dedicated `cursor-cli` parser.

### 2026-05-28 — PATH issue
Hooks fail with `altevra: command not found` because the binary lives in
`target/release/altevra` not in PATH. Resolved by symlink
`~/.local/bin/altevra` → release build. Future `altevra setup` wizard
should automate this on first run.

---

## 8. Quotes to hold onto

> *"Sve da se pamti, da misli baza, da misli, da obaveštava, da traži
> research, da skuplja vesti."*

> *"Da AI koji razmišlja bude mu lako, da embedding može da traži lako."*

> *"Da se knowledge i znanje i preferencije i sve o meni kompounduje
> kroz vreme."*

> *"Bukvalno pravim kroz godine ću napraviti digitalnu verziju sebe."*

> *"Stvarno želim da ovo napravim, da koristim godinama ovo, ne želim
> da napravim kao igračku, nego stvarno kao nešto jako ozbiljno, jako
> korisno, što ću koristiti svaki dan."*

> *"Ovo je stvarno jedan kontekst inženjering na jednom jako visokom
> nivou."*

---

**Last updated:** 2026-05-28
**Maintained by:** Pavle + Claude Code (Opus 4.7)
**Update protocol:** Append to "Decisions log" when something material
changes. Never rewrite the Vision section without Pavle's explicit
sign-off.

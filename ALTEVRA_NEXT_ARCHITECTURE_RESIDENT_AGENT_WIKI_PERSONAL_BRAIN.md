# ALTEVRA NEXT ARCHITECTURE — Resident Agent, Wiki Layer, Personal Brain & Context Engineering

**Owner:** Pavle Anđelković
**Project:** Altevra
**Status:** Architecture expansion / next build phase
**Adopted:** 2026-05-28
**Purpose:** Give Claude Code / Hermes / future agents a detailed source-of-truth document for the next major Altevra layer.

---

# 0. Core Direction

Altevra is not just a recorder, not just a memory database, and not just an MCP server.

Altevra is Pavle's local-first external mind and agent operating system.

The next phase must make Altevra capable of:

- thinking over its own context
- maintaining living wiki pages
- managing personal + professional knowledge equally
- running resident internal agents
- producing useful insights
- creating daily briefings
- keeping context fresh
- helping cheaper models perform well through strong context engineering
- compounding knowledge over years

The goal is not to build another AI productivity toy.

The goal is to build a long-term context intelligence system that gets more useful the longer it runs.

---

# 1. Main New Concepts

This phase adds the following major systems:

```txt
1. Resident Agent Layer
2. Agent Mode System
3. Context Packet System
4. Token Economy Rules
5. Model Routing Layer
6. Personal Brain Layer
7. Wiki Layer
8. Wiki Curator Agent
9. Relevance Gate
10. Daily Capture Protocol
11. Daily Briefing Agent
12. Memory Curator Agent
13. Synthesis Agent
14. Insight Agent
15. Observer Agent
16. Identity Seed System
17. Protected Memory Rules
18. Auto-Wiki Update Pipeline
```

These systems must integrate with existing Altevra primitives:

```txt
- events
- update_feed
- sessions
- skills
- hooks
- tasks
- goals
- research
- memory
- MCP
- CLI
- adapters
- secrets
```

---

# 2. Main Philosophy

Altevra must not load the whole vault into every prompt.

That would destroy token efficiency and make cheap models behave badly.

Instead, Altevra must use:

```txt
small permanent system prompt
+ mode-specific prompt
+ strict context packet
+ retrieval tools
+ output schemas
+ memory write rules
```

The Resident Agent should not be smart because it sees everything.

It should be smart because it knows:

```txt
what to ask
what to ignore
what to connect
what to remember
what to mark as uncertain
what to turn into action
```

---

# 3. Resident Agent Layer

## 3.1 Definition

The Resident Agent is the internal intelligence layer inside Altevra.

It lives inside the Altevra system and turns raw context into structured intelligence.

It is not a chat assistant.

It is not a general agent that does everything randomly.

It is a controlled internal reasoning system with modes.

## 3.2 Main Responsibilities

The Resident Agent should:

* read last updates
* inspect sessions
* analyze memory
* synthesize research
* generate insights
* maintain wiki pages
* update tasks/goals when appropriate
* detect stale information
* detect conflicts
* create review items
* create daily briefings
* propose memory writes
* preserve provenance
* minimize token usage

## 3.3 What It Must Never Do

The Resident Agent must not:

* overwrite source-of-truth files directly
* rewrite identity/profile without approval
* treat external research as confirmed truth
* send private/secret context to external models unless allowed
* load the full vault by default
* create random tasks from vague thoughts
* hallucinate missing context
* auto-delete memory
* turn Altevra into noisy research spam

---

# 4. Resident Agent Core Prompt

Create this file:

```txt
/06-skills/resident-agent-core.md
```

Content:

```md
# Altevra Resident Agent — Core System Prompt

You are the resident intelligence agent inside Altevra.

Altevra is Pavle's local-first external mind and agent operating system.

Your purpose is to help Altevra compound knowledge over time.

You turn memory, research, sessions, tasks, goals, updates, wiki pages, and decisions into useful structured intelligence.

You do not guess.
You retrieve.

You do not dump information.
You compress and structure.

You do not treat external research as truth.
You treat it as evidence.

You do not overwrite source-of-truth files.
You propose changes.

You do not use long context unless necessary.
You request minimal context first, then hydrate only what matters.

Your priorities are:

1. preserve truth
2. reduce noise
3. connect relevant information
4. create useful insights
5. update tasks/goals when needed
6. protect privacy
7. minimize token usage
8. improve future retrieval

Before acting, identify:

- current mode
- project
- goal
- task
- source type
- sensitivity
- required tools
- output format

When uncertain, create a knowledge gap instead of inventing.

Every useful output must include:

- summary
- reasoning basis
- sources used
- confidence
- recommended next action
- memory writes proposed
- tasks proposed
- review items proposed
- events to emit
```

---

# 5. Agent Mode System

Do not create one giant always-on god agent.

Create one Resident Agent with multiple modes.

Each mode has:

```txt
- mode name
- mode-specific prompt
- allowed tools
- forbidden tools
- context packet type
- output schema
- token budget
```

## 5.1 Required Modes

```txt
1. memory_curator
2. synthesis
3. daily_briefing
4. wiki_curator
5. insight
6. observer
7. research
8. task_goal
```

Start with the first four:

```txt
1. memory_curator
2. synthesis
3. daily_briefing
4. wiki_curator
```

Do not build all modes perfectly at once.

Build the mode routing system and first usable modes.

---

# 6. Mode Prompts

Create folder:

```txt
/06-skills/resident-agent-modes/
```

---

## 6.1 Memory Curator Mode

File:

```txt
/06-skills/resident-agent-modes/memory-curator.md
```

Prompt:

```md
# Mode: Memory Curator

Your job is to keep Altevra memory clean, deduplicated, current, and searchable.

You inspect:

- new notes
- old notes
- session summaries
- memories
- duplicates
- outdated files
- conflicting facts
- weak metadata
- category drift

Rules:

- Never delete automatically.
- Propose merges.
- Mark stale content.
- Prefer source-of-truth.
- Preserve provenance.
- Create review items when unsure.
- Do not rewrite confirmed personal facts.
- Do not merge sensitive personal records without review.
- Do not turn vague thoughts into confirmed facts.

Output JSON:

{
  "summary": "",
  "dedupe_suggestions": [],
  "stale_items": [],
  "conflicts": [],
  "metadata_updates": [],
  "category_suggestions": [],
  "review_items": [],
  "confidence": 0.0,
  "events_to_emit": []
}
```

---

## 6.2 Synthesis Mode

File:

```txt
/06-skills/resident-agent-modes/synthesis.md
```

Prompt:

```md
# Mode: Synthesis

Your job is to turn cleaned research or structured memory into project-linked insight.

You receive:

- project context summary
- active goals
- active tasks
- cleaned research items
- existing related knowledge
- recent updates
- relevant wiki pages

Do not process raw scraped pages directly.

External research is evidence, not truth.

Extract:

- key findings
- patterns
- opportunities
- risks
- contradictions
- useful examples
- recommended actions
- possible tasks
- memory writes

Create tasks only when the action is concrete.

Output JSON:

{
  "summary": "",
  "key_findings": [],
  "patterns": [],
  "opportunities": [],
  "risks": [],
  "contradictions": [],
  "linked_projects": [],
  "linked_wiki_pages": [],
  "recommended_actions": [],
  "proposed_tasks": [],
  "memory_writes": [],
  "review_items": [],
  "confidence": 0.0,
  "uncertainties": [],
  "events_to_emit": []
}
```

---

## 6.3 Daily Briefing Mode

File:

```txt
/06-skills/resident-agent-modes/daily-briefing.md
```

Prompt:

```md
# Mode: Daily Briefing

Your job is to create a short, useful daily brief from Altevra context.

Use:

- last updates
- active tasks
- active goals
- recent sessions
- important decisions
- useful research
- wiki updates
- personal notes if relevant and allowed

Do not include noise.

Do not create long summaries.

Prefer action over description.

Sections:

1. What changed
2. What matters
3. Decisions made
4. Tasks needing attention
5. Useful research
6. Risks
7. Personal signals if relevant
8. Suggested focus today

Output Markdown:

# Daily Brief — {date}

## What Changed

## What Matters

## Decisions

## Tasks Needing Attention

## Useful Research

## Risks

## Personal Signals

## Suggested Focus
```

---

## 6.4 Wiki Curator Mode

File:

```txt
/06-skills/resident-agent-modes/wiki-curator.md
```

Prompt:

```md
# Mode: Wiki Curator

Your job is to maintain living wiki pages inside Altevra.

A wiki page is a synthesized, human-readable, agent-readable explanation of a topic.

A wiki page is not a raw note.

You receive:

- topic name
- existing wiki page if it exists
- recent updates
- relevant memories
- decisions
- tasks
- goals
- research items
- related entities
- related wiki pages

Your job:

1. decide whether to create, update, split, merge, or leave the wiki page unchanged
2. preserve source links and provenance
3. update only sections affected by new evidence
4. keep the page concise
5. mark uncertainty clearly
6. never treat one weak source as confirmed truth
7. create open questions when context is incomplete
8. link related pages using [[wiki-links]]

Do not dump raw logs into wiki pages.

Do not rewrite the whole page unless the topic changed significantly.

Output JSON:

{
  "action": "create|update|split|merge|unchanged",
  "topic": "",
  "summary_of_change": "",
  "sections_changed": [],
  "new_links": [],
  "open_questions": [],
  "confidence": 0.0,
  "proposed_page_markdown": "",
  "review_required": false,
  "review_reason": "",
  "events_to_emit": []
}
```

---

## 6.5 Insight Mode

File:

```txt
/06-skills/resident-agent-modes/insight.md
```

Prompt:

```md
# Mode: Insight

Your job is to notice useful connections across memory, research, tasks, goals, sessions, and wiki pages.

You are not brainstorming randomly.

A valid insight must:

- connect at least 2 pieces of evidence
- relate to an active goal, project, risk, preference, or important life domain
- suggest a useful action, decision, or reflection
- include confidence and sources

Avoid obvious insights.

Output JSON:

{
  "insights": [
    {
      "title": "",
      "summary": "",
      "evidence": [],
      "why_it_matters": "",
      "recommended_action": "",
      "linked_projects": [],
      "linked_wiki_pages": [],
      "confidence": 0.0
    }
  ],
  "noise": [],
  "events_to_emit": []
}
```

---

## 6.6 Observer Mode

File:

```txt
/06-skills/resident-agent-modes/observer.md
```

Prompt:

```md
# Mode: Observer

Your job is to observe Altevra usage and propose improvements.

Analyze:

- repeated tool call patterns
- failed searches
- repeated questions
- stale tasks
- missing capabilities
- outdated skills
- high-cost model usage
- broken hooks
- bad retrieval
- noisy research
- recurring personal/productivity patterns

Do not interrupt work.

Do not auto-apply changes.

Output JSON:

{
  "skill_proposals": [],
  "capability_gaps": [],
  "knowledge_gaps": [],
  "cost_warnings": [],
  "process_improvements": [],
  "retrieval_improvements": [],
  "hook_issues": [],
  "events_to_emit": []
}
```

---

# 7. Context Packet System

The Resident Agent must not receive huge unstructured context.

Every run receives a strict context packet.

Create module:

```txt
crates/altevra-bootstrap/src/context_packet.rs
```

Or a new crate if cleaner:

```txt
crates/altevra-context/
```

## 7.1 Context Packet Shape

```json
{
  "packet_type": "resident_agent_context",
  "mode": "synthesis",
  "project": "altevra",
  "goal": {
    "id": "goal_altevra_context_os",
    "summary": "Build Altevra as a local-first external mind and agent OS"
  },
  "active_task": {
    "id": "task_design_resident_agent",
    "summary": "Design resident agent prompt and context engineering layer"
  },
  "relevant_context": {
    "source_of_truth": [
      {
        "id": "doc_claude_md",
        "title": "CLAUDE.md",
        "summary": "Altevra is Pavle's external mind and must compound across decades",
        "path": "CLAUDE.md"
      }
    ],
    "recent_updates": [],
    "related_decisions": [],
    "related_wiki_pages": [],
    "constraints": []
  },
  "inputs": [],
  "allowed_tools": [
    "search_memory",
    "get_chunk",
    "get_wiki_page",
    "save_insight",
    "create_task",
    "create_review_item",
    "emit_event"
  ],
  "forbidden_actions": [
    "overwrite_source_of_truth",
    "send_secret_to_external_tool",
    "load_full_vault",
    "direct_db_query"
  ],
  "output_schema": "synthesis_v1",
  "token_budget": {
    "max_input_tokens": 12000,
    "max_output_tokens": 1500
  }
}
```

## 7.2 Context Packet Rules

```txt
1. Always start with summaries, not full documents.
2. Hydrate full chunks only when needed.
3. Never load the full vault.
4. Prefer source-of-truth files.
5. Include last updates.
6. Include related wiki pages.
7. Include active goals/tasks.
8. Include sensitivity labels.
9. Include allowed tools.
10. Include output schema.
```

---

# 8. Token Economy Rules

Create file:

```txt
/00-system/token-economy.md
```

Content:

```md
# Altevra Token Economy Rules

Altevra should make cheap models useful by giving them clean, small, structured context.

Rules:

1. Always start with summaries, not full documents.
2. Hydrate full content only when needed.
3. Never load more than 5 full chunks at once.
4. Prefer source-of-truth files.
5. Ignore archived/deprecated content unless asked.
6. Compress repeated context.
7. Use IDs and summaries in first pass.
8. Use full text only in second pass.
9. Save outputs as structured data, not verbose prose.
10. If context is too large, ask Altevra to rerank.
11. Do not send raw internet scrape directly to strong reasoner.
12. Use cheap model for cleanup, strong model for synthesis.
13. Avoid generic insights.
14. Avoid long prose when structured JSON is enough.
```

---

# 9. Model Routing Layer

Altevra must be model-agnostic.

Do not hardcode one model provider.

Create roles:

```txt
cheap_worker
strong_reasoner
local_private
embedding_model
reranker
```

## 9.1 Role Definitions

```md
# Model Roles

## cheap_worker

Used for:

- text cleanup
- metadata extraction
- simple classification
- small summaries
- category detection
- boilerplate removal

## strong_reasoner

Used for:

- synthesis
- conflict detection
- architecture reasoning
- insight generation
- daily brief reasoning
- wiki page synthesis
- prioritization

## local_private

Used for:

- personal memories
- relationship notes
- health/mood notes
- identity data
- sensitive context

## embedding_model

Used for:

- vector search
- semantic retrieval

## reranker

Used for:

- ranking search results
- choosing chunks for hydration
```

## 9.2 Routing Rules

```txt
Raw research → cheap_worker cleanup → structured summaries → strong_reasoner synthesis

Private personal context → local_private if available

Embeddings → embedding_model

Search result ranking → reranker or internal scorer
```

## 9.3 Recommended Runtime Models

Altevra runtime should prefer cheap but strong reasoning models.

Examples:

```txt
- DeepSeek V4 Pro
- DeepSeek V4 Flash
- MiniMax M2.5 Pro
- MiniMax M2.7
- Gemini cheap/free fallback
- local model later for private docs
```

Expensive frontier models may be used for development/review, but Altevra must not depend on them.

---

# 10. Personal Brain Layer

This is central.

Altevra must treat personal and professional knowledge equally.

Do not make Altevra only about code/projects.

Create dedicated module:

```txt
crates/altevra-personal/
```

Or initially implement inside memory/wiki, but architecture should reserve this concept.

## 10.1 Personal Data Types

Altevra must support:

```txt
decision
learning
preference
person
relationship
place
idea
goal
mood
health
memory
reference
habit
routine
value
identity_shift
life_event
```

## 10.2 Personal Brain Rules

```txt
1. Personal data is first-class.
2. Business data is not more legitimate than personal data.
3. Personal memories need sensitivity labels.
4. Relationship notes require extra caution.
5. Identity/profile updates require review.
6. Confirmed preferences should be versioned.
7. Inferred preferences should be marked as inferred.
8. Personal data should not leave the machine unless explicitly allowed.
```

## 10.3 Identity Seed Files

Create:

```txt
~/.imperium/identity/profile.yaml
~/.imperium/identity/preferences.yaml
~/.imperium/identity/life-domains.yaml
~/.imperium/identity/active-goals.yaml
```

Example:

```yaml
name: Pavle Anđelković
role: Founder
system: Imperium
primary_projects:
  - Altevra
  - ReVesta
  - Tunia
  - ImperiumCrawl
life_domains:
  - business
  - relationships
  - health
  - learning
  - music
  - family
  - personal_growth
privacy_default: local_only
```

## 10.4 Personal Memory Write Schema

```json
{
  "type": "preference|learning|memory|person|relationship|goal|health|mood",
  "summary": "",
  "source": "",
  "confidence": "direct|inferred|weak",
  "sensitivity": "public|internal|private|secret",
  "created_at": "",
  "related_people": [],
  "related_projects": [],
  "review_required": true
}
```

---

# 11. Relevance Gate

Altevra must not research random nonsense.

It must only research what is useful.

Create:

```txt
crates/altevra-relevance/
```

Or module inside `altevra-research`.

## 11.1 Relevance Definition

Useful content is content that:

```txt
1. contributes to an active goal
2. deepens a stated interest
3. warns about a risk
4. improves a project
5. improves a personal system
6. updates a known important topic
```

## 11.2 Not Useful By Default

Filter out:

```txt
- random entertainment trivia
- low-value tech hype
- shallow content
- duplicate research
- ego scrolling
- unrelated trends
- topics not connected to active goals/interests
```

## 11.3 Relevance Gate Output

```json
{
  "item_id": "",
  "decision": "keep|discard|review",
  "reason": "",
  "linked_goal": "",
  "linked_project": "",
  "linked_interest": "",
  "confidence": 0.0
}
```

---

# 12. Wiki Layer

This is one of the most important new additions.

Altevra needs a living wiki system.

The wiki is the synthesized, human-readable, agent-readable knowledge layer.

## 12.1 Definition

A wiki page is not a raw note.

A wiki page is what Altevra currently understands about a topic.

```txt
Event log = what happened
Memory = what was stored
Insight = what was noticed
Wiki page = what is currently understood
```

## 12.2 Why Wiki Matters

Without wiki pages, agents must synthesize from raw context every time.

With wiki pages, agents can load a compact page that explains a concept, project, person, pattern, or decision.

This saves tokens and improves reasoning.

## 12.3 Wiki Folder Structure

Create:

```txt
/wiki/
  /projects/
    altevra.md
    revesta.md
    tunia.md

  /concepts/
    agent-os.md
    context-engineering.md
    resident-agent.md
    skill-bootstrap.md
    last-updates-feed.md
    wiki-layer.md
    personal-brain-layer.md

  /people/
    pavle.md
    danilo.md
    andrija.md

  /patterns/
    overnight-agent-runs.md
    research-synthesis-loop.md
    daily-briefing-loop.md

  /decisions/
    rust-first-cli-first.md
    source-available-license.md

  /domains/
    ai-agents.md
    foreclosure-surplus.md
    vocal-coaching.md
```

## 12.4 Wiki Frontmatter

```md
---
id: wiki_context_engineering
type: wiki_page
topic: context_engineering
status: living
confidence: medium
last_synthesized_at: 2026-05-28
source_count: 17
related_projects:
  - altevra
related_pages:
  - resident-agent
  - skill-bootstrap
  - last-updates-feed
sensitivity: internal
owner: altevra
---
```

## 12.5 Wiki Page Template

```md
# {Topic}

## Current Understanding

Short explanation of what Altevra currently believes about this topic.

## Why It Matters

Why this topic matters for Pavle, a project, a goal, or a decision.

## Key Facts

- Fact 1
- Fact 2
- Fact 3

## Key Decisions

- Decision with date and source
- Decision with date and source

## Open Questions

- Question 1
- Question 2

## Related Projects

- Project A
- Project B

## Related People

- Person A
- Person B

## Related Pages

- [[other-topic]]
- [[another-topic]]

## Evidence / Sources

- source id / document / session / research item

## Last Updates

- Recent change 1
- Recent change 2

## Suggested Next Actions

- Action 1
- Action 2
```

---

# 13. Auto-Wiki Pipeline

Altevra should automatically maintain wiki pages.

## 13.1 Pipeline

```txt
new event / session / research / decision
→ classify candidate topics
→ match existing wiki pages
→ decide create/update/no-op
→ generate diff
→ save pending wiki update
→ auto-apply low-risk update
→ require review for sensitive/source-of-truth pages
→ emit event
```

## 13.2 Wiki Update Policy

Auto-apply allowed:

```txt
- low-risk concept updates
- project summaries from public/internal data
- research summary sections
- related links
- open questions
```

Review required:

```txt
- identity profile changes
- relationship/person pages
- source-of-truth decisions
- sensitive personal facts
- major rewrites
- conflicting evidence
```

## 13.3 Wiki CLI Commands

Add:

```bash
altevra wiki list
altevra wiki show <topic>
altevra wiki search <query>
altevra wiki suggest --since 24h
altevra wiki synthesize --topic <topic>
altevra wiki update <topic>
altevra wiki graph
```

## 13.4 Wiki MCP Tools

Add:

```txt
get_wiki_page(topic)
search_wiki(query)
suggest_wiki_updates(since)
update_wiki_page(topic, mode)
get_related_wiki_pages(topic)
```

---

# 14. Example Wiki Page: Resident Agent

Create:

```txt
/wiki/concepts/resident-agent.md
```

Content:

```md
---
type: wiki_page
topic: resident_agent
status: living
confidence: high
related_projects:
  - altevra
related_pages:
  - context-engineering
  - model-routing
  - memory-curator
---

# Resident Agent

## Current Understanding

The Resident Agent is the internal intelligence layer inside Altevra.

It turns memory, research, sessions, tasks, goals, wiki pages, decisions, and updates into structured intelligence.

It should not be one giant always-on agent. It should run in modes:

- memory_curator
- synthesis
- daily_briefing
- insight
- observer
- wiki_curator

## Why It Matters

This is the layer that makes Altevra think instead of only store.

Without it, Altevra is a recorder.

With it, Altevra becomes a compounding external mind.

## Key Decisions

- Use small core system prompt plus mode-specific prompts.
- Use context packets instead of dumping the vault.
- Use cheap models for cleanup and strong cheap reasoners for synthesis.
- Never send raw internet garbage directly to strong reasoner.
- Protect source-of-truth and identity files.

## Open Questions

- Which model should be default for synthesis?
- How often should Wiki Curator run?
- Which pages can be auto-updated without review?
- How much personal context should daily briefing include by default?

## Related Pages

- [[context-engineering]]
- [[model-routing]]
- [[daily-briefing]]
- [[wiki-layer]]
```

---

# 15. Daily Capture Protocol

Altevra needs a daily input/output loop.

If it does not run daily, it will not compound.

Create:

```txt
/00-system/daily-capture-protocol.md
```

## 15.1 Daily Input Questions

```md
# Daily Capture

Every day, capture:

1. What did Pavle work on today?
2. What did he learn?
3. What decision was made?
4. What changed?
5. What should happen tomorrow?
6. Was anything personally important?
7. What should Altevra remember?
8. Did any relationship/person context change?
9. Did any goal change?
10. Did any project status change?
```

## 15.2 Daily Output Brief

Altevra should produce:

```txt
- what changed
- what matters
- active tasks
- decisions
- risks
- useful research
- personal signals
- suggested focus
```

---

# 16. Protected Memory Rules

Create:

```txt
/00-system/protected-memory-rules.md
```

Content:

```md
# Protected Memory Rules

Altevra must protect sensitive and source-of-truth memory.

## Agent may write directly

- research item
- insight
- proposed task
- low-risk wiki update
- category suggestion
- review item
- event
- update feed item

## Agent may propose but not directly write

- source-of-truth docs
- identity profile
- confirmed preferences
- relationship notes
- sensitive personal facts
- decisions log
- health/mood patterns
- license/legal docs

## Required behavior

For protected content:

1. propose diff
2. create review item
3. mark sensitivity
4. wait for approval
```

---

# 17. Tooling Needed

## 17.1 Resident Agent CLI

Add:

```bash
altevra resident run --mode daily_briefing
altevra resident run --mode synthesis
altevra resident run --mode memory_curator
altevra resident run --mode wiki_curator
altevra resident modes
altevra resident prompt --mode synthesis
```

## 17.2 Resident Agent MCP

Add:

```txt
run_resident_agent
get_resident_agent_modes
get_resident_agent_prompt
get_context_packet
```

## 17.3 Context CLI

Add or extend:

```bash
altevra context packet --mode synthesis --project altevra
altevra context hydrate --ids <ids>
```

---

# 18. Output Schemas

Create folder:

```txt
/00-system/schemas/
```

Required schemas:

```txt
synthesis_v1.json
memory_curator_v1.json
daily_briefing_v1.md
wiki_curator_v1.json
insight_v1.json
observer_v1.json
relevance_gate_v1.json
personal_memory_v1.json
context_packet_v1.json
```

---

# 19. Data Model Additions

## 19.1 wiki_pages

```sql
wiki_pages
- id uuid primary key
- topic text not null
- slug text not null
- path text not null
- status text not null
- confidence text not null
- sensitivity text not null
- source_count int not null
- last_synthesized_at timestamptz null
- created_at timestamptz not null
- updated_at timestamptz not null
```

## 19.2 wiki_page_links

```sql
wiki_page_links
- id uuid primary key
- from_page_id uuid not null
- to_page_id uuid not null
- link_type text not null
- created_at timestamptz not null
```

## 19.3 resident_agent_runs

```sql
resident_agent_runs
- id uuid primary key
- mode text not null
- project_id uuid null
- task_id uuid null
- model_role text not null
- model_provider text null
- input_tokens int null
- output_tokens int null
- cost_usd numeric null
- status text not null
- started_at timestamptz not null
- finished_at timestamptz null
- result_summary text null
```

## 19.4 personal_memory

```sql
personal_memory
- id uuid primary key
- memory_type text not null
- summary text not null
- source text not null
- confidence text not null
- sensitivity text not null
- review_required bool not null
- created_at timestamptz not null
- updated_at timestamptz not null
```

## 19.5 relevance_decisions

```sql
relevance_decisions
- id uuid primary key
- item_id uuid not null
- decision text not null
- reason text not null
- linked_goal_id uuid null
- linked_project_id uuid null
- linked_interest text null
- confidence real not null
- created_at timestamptz not null
```

---

# 20. Build Order

Do not build everything randomly.

Build in this order:

## Phase 1 — Prompt and Schema Foundation

```txt
1. Add resident-agent core prompt.
2. Add mode prompts:
   - memory_curator
   - synthesis
   - daily_briefing
   - wiki_curator
3. Add token economy doc.
4. Add protected memory rules.
5. Add context packet schema.
6. Add output schemas.
```

## Phase 2 — Wiki Layer Skeleton

```txt
1. Create /wiki folder structure.
2. Add wiki frontmatter parser.
3. Add wiki page template.
4. Add wiki_pages table.
5. Add CLI:
   - wiki list
   - wiki show
   - wiki search
6. Add example pages:
   - resident-agent.md
   - context-engineering.md
   - wiki-layer.md
```

## Phase 3 — Resident Agent Runtime

```txt
1. Add resident run command.
2. Add mode selection.
3. Add context packet generation.
4. Add output schema validation.
5. Add model routing role selection.
6. Log resident_agent_runs.
```

## Phase 4 — Wiki Curator

```txt
1. Add topic classifier.
2. Add suggest wiki updates.
3. Add synthesize topic.
4. Add diff/review behavior.
5. Auto-apply only low-risk updates.
```

## Phase 5 — Personal Brain Layer

```txt
1. Add identity seed files.
2. Add personal memory schema.
3. Add personal memory write rules.
4. Add relevance gate.
5. Add onboarding questions later.
```

## Phase 6 — Daily Briefing

```txt
1. Pull last updates.
2. Pull tasks/goals.
3. Pull important session summaries.
4. Pull useful research.
5. Pull wiki changes.
6. Generate daily brief.
7. Save to journal.
```

---

# 21. First Implementation Task For Claude Code

Use this exact task.

```md
# Task: Build Altevra Resident Agent + Wiki Foundation

We are adding the next major Altevra layer.

Do not build everything.

Build the foundation for:

- Resident Agent
- Mode prompts
- Context packet schema
- Token economy rules
- Wiki Layer
- Wiki Curator mode
- Personal Brain architecture docs

## Must create files

1. /06-skills/resident-agent-core.md
2. /06-skills/resident-agent-modes/memory-curator.md
3. /06-skills/resident-agent-modes/synthesis.md
4. /06-skills/resident-agent-modes/daily-briefing.md
5. /06-skills/resident-agent-modes/wiki-curator.md
6. /00-system/token-economy.md
7. /00-system/protected-memory-rules.md
8. /00-system/daily-capture-protocol.md
9. /00-system/schemas/context_packet_v1.json
10. /wiki/concepts/resident-agent.md
11. /wiki/concepts/context-engineering.md
12. /wiki/concepts/wiki-layer.md
13. /wiki/projects/altevra.md

## Must implement code

1. Basic wiki page parser
2. Basic wiki page listing
3. CLI:
   - altevra wiki list
   - altevra wiki show <topic>
   - altevra resident modes
   - altevra resident prompt --mode <mode>
4. Context packet struct
5. Output schema structs or validation placeholders
6. Resident mode enum
7. Tests for:
   - mode prompt loading
   - wiki page parsing
   - context packet serialization
   - protected memory rules file exists

## Do not build yet

- full autonomous resident agent
- full model calls
- full auto-wiki updater
- full personal brain DB
- dashboard
- notifications
- complex graph visualizations

## Rules

- Keep everything modular.
- Do not break existing CLI.
- Do not hardcode one model provider.
- Do not send personal data to cloud models by default.
- Wiki pages are synthesized knowledge, not logs.
- Resident Agent uses minimal context first.
- Protected files require review.
```

---

# 22. Success Criteria

This phase is successful when:

```txt
1. Altevra has resident agent prompt files.
2. Altevra has mode-specific prompts.
3. Altevra can list resident modes.
4. Altevra can show the prompt for a mode.
5. Altevra has /wiki structure.
6. Altevra can list wiki pages.
7. Altevra can show a wiki page by topic.
8. Context packet schema exists.
9. Token economy rules exist.
10. Personal Brain architecture is documented.
11. Claude Code understands this is the next source-of-truth.
```

---

# 23. Final Design Principle

Altevra should become useful daily before it becomes huge.

The core daily loop is:

```txt
capture → classify → update memory → update wiki → synthesize insight → brief Pavle → guide agents
```

If this loop works, Altevra is alive.

If this loop does not work, Altevra is just an impressive Rust archive.

Build the living loop first.

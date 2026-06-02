# Vault Document Template — universal frontmatter + atomization contract

Status: **canonical** (P0 atomization layer)
Date: 2026-06-02
Refs: **RECONCILIATION R13** (template + mandatory tags), R12 (tag/structured + FTS5 retrieval), R3 (governed domains), R1 (sensitivity ladder).

> Purpose: define the ONE frontmatter contract every vault markdown doc carries,
> the folder→type/domain map that fills it, and the rule that turns a "living"
> aggregate (`Decisions.md`, `Learnings.md`, `People.md`, a daily note) into many
> atomic, individually-recallable objects. Structure + governed tags ARE the
> search substrate (R12/R13) — no structure → not findable.

---

## 1. Why this exists

Pavle keeps ~6–8 canonical living markdown files he appends to. The human writes
few files; Altevra must see many atomic objects. Two complementary moves:

1. **Document-level normalization** (`altevra vault normalize`) — give *every*
   `*.md` a universal frontmatter envelope so the whole vault is uniformly typed,
   tagged, and machine-findable. SAFE: adds/merges frontmatter only, never edits
   the body, never deletes. DRY-RUN by default; `--apply` backs up the whole vault
   first.
2. **Section atomization** (`altevra capture --atomize`) — split a living
   aggregate into its `## ` sections and persist each as its OWN durable object
   (decision / learning / person / note), so recall returns the exact section, not
   the whole file.

---

## 2. Universal frontmatter contract

Every normalized document carries these keys. Normalize FILLS only the *missing*
ones; any pre-existing key (and its value) is **preserved verbatim**.

| Key | Meaning | Default when missing |
|-----|---------|----------------------|
| `type` | object type (see §3 map) | inferred from folder |
| `domain` | governed domain (R3) | inferred from folder |
| `sensitivity` | R1 ladder (`public`<`shareable`<`internal`<`confidential`<`secret`<`restricted`) | `internal`; **`restricted`** for a high-water domain |
| `status` | `active` \| `archived` | `archived` under `Archive/`, else `active` |
| `tags` | governed taxonomy (TAG-1: ≥1) | seed `[<domain>]` if empty/absent |
| `created` | first-seen date (`YYYY-MM-DD`) | existing value, else file mtime date |
| `updated` | last-touched date | file mtime date |
| `source` | provenance origin | `obsidian` |
| `altevra_normalized` | normalize marker (idempotency) | `true` |
| `scope` | project name (only `Projects/<P>/*`) | `<P>` |

**High-water domains** (R3) — `personal, relationship, health, legal, financial,
client` — force `sensitivity: restricted` so personal docs can't default-down and
leak. Sensitivity only ever rises (R1 `combine` = `max(level)`).

**Idempotency:** a doc already carrying `altevra_normalized: true` with every
universal field present yields no change on a re-run (only a newer file mtime
bumps `updated`).

---

## 3. Folder → type / domain map

Resolved by `altevra-vault::classify_path` (vault-relative path):

| Folder | `type` | `domain` | Notes |
|--------|--------|----------|-------|
| `Daily/*` | `daily_brief` | `business` | |
| `Memory/Decisions*` | `decision` | `business` | |
| `Memory/Learnings*` | `learning` | `business` | |
| `Memory/People*` / `Person*` | `person` | `relationship` | high-water → `restricted` |
| `Memory/*` (other) | `note` | `business` | |
| `Projects/<P>/*` | `note` | `project` | `scope = <P>` |
| `Wiki/*` or `Library/Wiki/*` | `wiki_page` | `business` | |
| `Ideas/*` | `idea` | `business` | |
| `Research/*` | `research` | `business` | |
| `Content/*` | `content` | `business` | |
| `System/*` | `reference` | `business` | |
| `Archive/*` | *inner type* | *inner domain* | `status: archived`; type/domain inferred from the path inside `Archive/` |
| else | `note` | `business` | |

**Excluded from normalization:** `Templates/` (seed docs copied FROM), any
`*/templates/*`, `node_modules`, `.obsidian/`. Non-UTF-8 files are skipped and
reported (never corrupted).

The builtin R13 templates (`TemplateRegistry::with_builtins`) cover
`decision, learning, person, wiki_page, daily_brief, preference, insight_card,
skill, hook`; the folder map above is the document-level projection of those types
onto the Imperium vault layout.

---

## 4. Atomization rule (`## section = object`)

`altevra-vault::parse_sections` splits a markdown aggregate into level-2 sections:

- **Boundary = `## ` only.** `#` (title), `###` and deeper are NOT boundaries — a
  level-2 section owns everything beneath it, including its `###` sub-headings.
- **Preamble is not a section.** Text before the first `## ` (the `# Title` + any
  intro prose) is the document preamble; it never becomes an object.
- **Body = up to the next `## `.** Leading/trailing blank lines trimmed; interior
  blanks kept.
- **Empty-body sections are skipped** — no empty objects.
- **Date:** the first `YYYY-MM-DD` found anywhere in the heading (e.g.
  `## 2026-06-02 — ReVesta validated`) becomes the object's created date; an
  invalid calendar date or none → no date.

### Capture path (`altevra capture --atomize`)

For each section:

1. **type** inferred from the filename — `Decisions→decision`, `Learnings→learning`,
   `People→person`, else `note` — stored as a `kind:<type>` tag/category (the row
   itself is a `learning` envelope; the `kind` tag carries the atomized type).
2. **domain** inferred from the file path (`infer_domain`); high-water → sensitivity
   escalated to `restricted`.
3. **body** runs through `guard_text` (secret + PII redaction). A credential-class
   (`rejected`) sighting — PEM key / credentialed DB URL — **skips that section**
   with a warning; all other sections still capture.
4. **id** = `capture-<filestem>-<section-slug>-<8charhash>` (stable; the slug drops
   a leading date so it carries the topic).
5. Inserted as a `LearningRow` → auto-indexed into `object_index` + `object_fts`
   (T-INV14), so the section is immediately recallable by its own unique words.

**Auto-atomize** fires when a file is a known aggregate (`Decisions`/`Learnings`/
`People`, or any file under `Memory/`) AND has ≥2 `## ` sections. `--atomize`
forces it on; `--no-atomize` forces the legacy whole-file path (back-compat).

---

## 5. Section-level template (per-type) — "ČAK I DELOVE u dokumentu"

Beyond the document frontmatter, each `## ` SECTION must follow a per-type
contract of **bold-label fields**. `altevra-vault::section_template` defines these,
**calibrated against the real `Memory/*.md`** (not invented). A label may be
block-level (`**Odluka:** …`) or a list item (`- **Odluka:** …`); a slot is
satisfied by ANY of its synonyms followed by a non-empty value. A slot is empty if
the label is a bare stub (`**Odluka:**` with nothing after it).

| Type | Required slots (synonyms) | Optional slots | Style |
|------|---------------------------|----------------|-------|
| **decision** | `Odluka`/`Decision` · `Zašto`/`Šta znači`/`Razlog`/`Why`/`Šta to znači u praksi`/`Filozofija` | `Pravilo za primenu`/`Pravilo`/`Sledeći korak`/`Next action`/`Operating model`/`How to apply` | strict |
| **person** | `Kontekst`/`Context` · `Uloga`/`Status`/`Commitment`/`Obećanje` | `Relevance`/`Relevantnost`/`Fokus`/`Tema` | strict |
| **learning** | *(none — freeform)* | `Lekcija`/`Learning`/`Insight` · `Primena`/`Fix`/`Preporuka`/`Preventivno` | freeform (non-empty) |
| **daily_brief / note** | *(none)* | — | freeform (non-empty) |

Calibration basis: `**Odluka:**` appears in 24/31 real decision sections; the
"why" slot is a synonym set because Pavle uses several; `Learnings.md` sections are
mostly plain prose (only 4/16 carry a label) → learning is **freeform**, never
forced into labels it doesn't use.

### Conformance + scaffolding flow

- `section_conformance(section, type) → { conformant, missing_labels, present_optional, empty }`.
- `altevra vault normalize` (DRY-RUN) reports per file: how many sections are
  non-conformant, which required labels are missing, and splits them into:
  - **scaffoldable** — empty/bare-stub sections (safe to fill with the skeleton now);
  - **need_rewrite** — sections with PROSE but missing labels (the LLM `--rewrite`
    job; never auto-edited in Phase 1).
- `altevra vault normalize --scaffold-empty` (apply) fills ONLY scaffoldable
  sections with `scaffold_section(type)` (canonical labels, blank values). It
  **never** rewrites a prose section — zero content loss. Backup-first + idempotent,
  same as frontmatter normalization.
- `altevra capture --atomize` tags each atomized object `conformant` or
  `needs-structure`, so recall can surface "this note needs cleanup."

### Phase 2 — LLM restructure seam (gated, not auto-run)

`altevra vault normalize --rewrite` is the path for **need_rewrite** sections:
it asks the configured reasoning provider (`altevra_llm::build_router`; live
`codex_oauth` GPT-5.5 / `api` mode) to RESTRUCTURE existing free prose into the
section template **without losing any fact** (output must contain every input
fact). Under the default `reasoning_mode = "delegated"` it is a **no-op that only
reports** "would rewrite N sections (needs codex_oauth/api)". Real LLM rewrites on
the vault are left to Pavle (DRY-RUN/report only by default).

---

## 6. Why this is the search substrate (R12 / R13)

Core retrieval is vector-free: tag/structured filters + FTS5 BM25 + graph (R12).
That makes the frontmatter envelope (type/domain/status/sensitivity/tags) and the
atomized `## ` objects the load-bearing way both Pavle's agents and Altevra's
resident modes find things. TAG-1 (≥1 governed category, always) + TEMPLATE-1
(faced types satisfy their template) guarantee nothing is stored "smuljano" /
unfindable. The opt-in BGE-M3 hybrid layer (R15) sits ABOVE this core and never
changes it.

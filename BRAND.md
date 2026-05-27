# Altevra — Brand Guidelines

**Version:** 1.0 — 2026-05-27
**Owner:** Pavle Anđelković

---

## 1. Name

**Altevra** — pronounced *al-TEV-ra* (Serbian/English equivalent).

The name has no public etymology. It is a coined word designed to be:
- **Distinct** — no Google ambiguity, no trademark conflicts as of 2026-05-27
- **Pronounceable** — works in Serbian, English, German, Spanish, Japanese
- **Infrastructure-grade** — sits in the same naming family as *Vercel, Datadog, Supabase, Vespera, Tessera*

**Do not use:**
- "AltEvra" (camel-case)
- "ALTEVRA" (all caps, except in ASCII headers)
- "Altevra AI" (no descriptive suffix)
- "Project Altevra" (no project- prefix)

**Always:** **Altevra** (capital A, rest lowercase).

---

## 2. Tagline

**Primary:** *The omniscient brain layer for your AI tools.*

**Variations** (context-dependent):
- *What your tools forget, Altevra remembers.*
- *Local-first AI memory infrastructure.*
- *One brain. Every tool. Your laptop.*

**Do not pair Altevra with:**
- "Productivity tool" — too generic
- "AI assistant" — Altevra is *infrastructure*, not an assistant
- "Note-taking app" — different category entirely

---

## 3. Color Palette — Crimson Infrastructure

Altevra's visual identity is built on a **deep crimson** palette. The dominant red signals: blood, signal, alert, intensity, depth. Altevra is a **serious infrastructure tool**, not a friendly consumer app.

### Primary

| Token | Hex | RGB | Use |
|-------|------|-----|-----|
| **`altevra-red`** | `#DC2626` | `220, 38, 38` | Logo, primary buttons, brand accents |
| **`altevra-red-deep`** | `#7F1D1D` | `127, 29, 29` | Hover states, header backgrounds |
| **`altevra-red-blood`** | `#450A0A` | `69, 10, 10` | Dark mode primary, depth fills |

### Supporting

| Token | Hex | Use |
|-------|------|-----|
| `altevra-coal` | `#0A0A0A` | Background (dark mode default) |
| `altevra-ash` | `#1A1A1A` | Card backgrounds, code blocks |
| `altevra-iron` | `#3F3F46` | Borders, dividers |
| `altevra-bone` | `#FAFAFA` | Primary text on dark backgrounds |
| `altevra-ember` | `#FCA5A5` | Highlight text, soft accents |

### Semantic

| Token | Hex | Use |
|-------|------|-----|
| `altevra-success` | `#16A34A` | Pass states, tests green |
| `altevra-warn` | `#EAB308` | Warnings, drift, attention |
| `altevra-error` | `#DC2626` | (Reuses brand red — errors are first-class) |

**Mandate:** Altevra UI is **dark-mode-first**. Light mode exists but is treated as a fallback, not the canonical experience.

---

## 4. Typography

**Code & technical content:** `JetBrains Mono`, fallback `Cascadia Code`, fallback `monospace`.

**Body text:** `Inter`, fallback system-ui.

**Display / headers:** `Inter` weight 700-900, letter-spacing -0.02em.

**ASCII logo / banners:** Block-font (`ANSI Shadow` style — see README header).

---

## 5. Logo

The Altevra logomark is the ASCII block-text rendering of "ALTEVRA" in `ANSI Shadow` font, **rendered in `altevra-red`** when terminal supports color, and `altevra-coal` on light backgrounds.

```
█████╗ ██╗  ████████╗███████╗██╗   ██╗██████╗  █████╗
██╔══██╗██║  ╚══██╔══╝██╔════╝██║   ██║██╔══██╗██╔══██╗
███████║██║     ██║   █████╗  ██║   ██║██████╔╝███████║
██╔══██║██║     ██║   ██╔══╝  ╚██╗ ██╔╝██╔══██╗██╔══██║
██║  ██║███████╗██║   ███████╗ ╚████╔╝ ██║  ██║██║  ██║
╚═╝  ╚═╝╚══════╝╚═╝   ╚══════╝  ╚═══╝  ╚═╝  ╚═╝╚═╝  ╚═╝
```

**Mini-mark (for favicons, badges, small contexts):**

```
 █████╗
██╔══██╗
███████║
██╔══██║
██║  ██║
╚═╝  ╚═╝
```

A capital **A** in `altevra-red` on `altevra-coal` background, with a single horizontal slash representing the "altevra signal cut" — the moment Altevra captures something other tools would lose.

---

## 6. CLI banner & terminal output

Every `altevra` command MAY display a one-line crimson-accent banner at startup. The banner uses ANSI escape `\x1b[1;38;5;160m` (bright crimson) on the `ALTEVRA` token, followed by version in dim white.

```
▌ ALTEVRA 0.3-alpha  •  local-first AI memory
```

**Rule:** Never use rainbow output, never use pure cyan/magenta/yellow as primary accent. Stay in the crimson family. The terminal is dark, Altevra is dark, the accent is blood.

---

## 7. Voice & messaging

Altevra speaks in **short, declarative sentences**. No marketing fluff. No "revolutionary AI-powered productivity suite."

**Good:**
- "Altevra remembers everything your AI tools forget."
- "Local. Source-available. Built in Rust."
- "Your laptop. Your data. Your control."

**Bad:**
- "Unleash the power of AI memory orchestration with our intelligent platform."
- "Synergize your workflow with Altevra's revolutionary brain."
- "The future of AI is here."

When Altevra describes itself, it describes **what it does**, not how it makes you feel.

---

## 8. License & commercial position

Altevra is **source-available** under [PolyForm Strict 1.0.0](./LICENSE), not open-source in the OSI sense. The visual identity reinforces this:

- The deep crimson signals *serious infrastructure*, not friendly community software
- The "commercial license required" badge is always visible in README
- The license shield uses `#7F1D1D` (deepest red) — same color as the "commercial" badge

When writing about Altevra publicly:
- **Always say** "source-available" — never "open-source"
- Always link to LICENSE when discussing usage
- Always mention commercial licensing path for business users

---

## 9. Trademarks

"Altevra" and the Altevra logomark are unregistered trademarks owned by **Pavle Anđelković**. Trademark registration is planned for v1.0.

Until registration, common-law trademark protection applies. Unauthorized use of the Altevra name or logomark in a commercial context will be pursued.

---

## 10. Changelog

| Version | Date | Changes |
|---------|------|---------|
| 1.0 | 2026-05-27 | Initial brand guidelines. Crimson palette, ASCII logo, PolyForm Strict licensing position. |

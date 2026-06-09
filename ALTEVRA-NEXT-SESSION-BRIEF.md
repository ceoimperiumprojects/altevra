# ALTEVRA DEEP BUILD — Briefing za Novu Sesiju
**Datum kreiranja:** 2026-06-07  
**Autor:** Pavle Anđelković + Claude (prethodna sesija)  
**Cilj:** Ova sesija treba da izgradi Altevra u production-ready sistem koji snima apsolutno sve, pravi insajte, i integriše se sa Voice Gateway-em.

---

## 🧠 Kontekst — Šta je Altevra

Altevra je Pavlov lokalni AI OS / second brain. **Nije chatbot. Nije notes app.**  
To je sistem koji snima SVE — svaki prompt, tool call, odluku, fajl promenu — i iz toga pravi long-term intelligence koji se kompounduje kroz godine.

**Vizija (Pavlovim rečima):**
> "Bukvalno pravim kroz godine digitalnu verziju sebe. Sve da može da stane u ovu bazu — ceo moj život, biznis, zabava, sve."

**Binarni:** `/home/pavle/projekti/ai-tooling/altevra/target/release/altevra`  
**Vault (ispravni):** `/home/pavle`  
**DB:** `/home/pavle/.altevra/altevra.db`  
**Brain log:** `/home/pavle/.altevra/brain.log`  
**Source:** `/home/pavle/projekti/ai-tooling/altevra/`

---

## ✅ Šta je urađeno u PRETHODNOJ sesiji (2026-06-07)

### Fix 1: Vault path je bio pogrešan
- **Bio:** `--vault /home/pavle/projekti/ai-tooling/altevra` (dev folder, prazna DB)
- **Fix:** Promenjen u `--vault /home/pavle` u `/home/pavle/.claude.json`
- **Rezultat:** Altevra sada čita pravu bazu u `~/.altevra/altevra.db`

### Fix 2: Claude hooks instalirani
Pokrenuto: `cd /home/pavle && altevra install-hooks --tool claude-code`

Hooks koji sada postoje u `~/.claude/settings.json`:
```
UserPromptSubmit  → altevra hook-handle user_prompt_submit --tool claude-code
PreToolUse        → altevra hook-handle pre_tool_use --tool claude-code  
PostToolUse       → altevra hook-handle post_tool_use --tool claude-code
SessionStart      → altevra hook-handle session_start --tool claude-code --project $ALTEVRA_PROJECT
Stop              → altevra hook-handle session_end --tool claude-code
```

### Fix 3: Hermes sesije importovane
Pokrenuto: `cd /home/pavle && altevra import --tool hermes`  
Rezultat: **9 sesija, 1099 turns** importovano iz `~/.hermes/sessions/*.jsonl`

### Fix 4: Brain daemon pokrenut
```bash
cd /home/pavle && altevra brain start &
```
Status: radi, tick svakih 30s. **Ali nije persistent — ugasi se sa terminalom.**

### Stanje DB sada:
- **1157 turns, 10 sesija** u `~/.altevra/altevra.db`
- Sources: Hermes (1099) + Claude Code sesija od juče (58)

---

## 🔴 PROBLEMI KOJI TREBA REŠITI (prioritizovano)

### Problem 1 — KRITIČAN: Brain daemon nije persistent ⚠️
Brain daemon se ugasi kad se terminal zatvori. Mora da radi uvek u pozadini kao systemd service.

**Fix koji treba implementirati:**
```bash
# Napravi systemd user service fajl
mkdir -p ~/.config/systemd/user/
cat > ~/.config/systemd/user/altevra-brain.service << 'EOF'
[Unit]
Description=Altevra Brain Daemon
After=default.target

[Service]
Type=simple
WorkingDirectory=/home/pavle
ExecStart=/home/pavle/projekti/ai-tooling/altevra/target/release/altevra brain start
Restart=always
RestartSec=5
StandardOutput=append:/home/pavle/.altevra/brain.log
StandardError=append:/home/pavle/.altevra/brain.log

[Install]
WantedBy=default.target
EOF

systemctl --user enable altevra-brain
systemctl --user start altevra-brain
systemctl --user status altevra-brain
```

### Problem 2 — VISOK: MCP server nije persistent
Altevra MCP server koji Claude koristi se pokreće od strane Claude Code kada otvoriš sesiju. Ali konfig u `~/.claude.json` kaže pogrešan vault.

**Verifikuj da je fix prošao:**
```bash
python3 -c "
import json
with open('/home/pavle/.claude.json') as f:
    d = json.load(f)
print('Vault arg:', d['mcpServers']['altevra']['args'])
# Treba biti: ['serve', '--vault', '/home/pavle']
"
```

Ako nije ispravno, popravi:
```python
# Isti fix kao prošli put
d['mcpServers']['altevra']['args'] = ['serve', '--vault', '/home/pavle']
```

### Problem 3 — VISOK: Observer insights vraćaju 0
`altevra observer` treba da detektuje patterns (drift, stale projects, decision conflicts...) ali vraća prazno jer:
- Observer nije imao dovoljno podataka (sada ima 1157 turns)
- Observer job možda nije konfigurisan pravilno

**Fix:**
```bash
cd /home/pavle && altevra observer --help  # vidi opcije
cd /home/pavle && altevra brain tick       # ručno pokreni jedan tick
cd /home/pavle && altevra observer         # proveri insights
```

### Problem 4 — SREDNJI: Claude Code sesije se ne indeksiraju odmah
Hook-ovi su instalirani ali treba verifikovati da zapravo pišu turns u DB.

**Verifikacija posle restarta Claude:**
```bash
# Posle jednog prompta u novoj sesiji, provjeri:
sqlite3 /home/pavle/.altevra/altevra.db \
  "SELECT COUNT(*), MAX(created_at) FROM turns WHERE tool = 'claude-code';"
# Broj treba da raste
```

### Problem 5 — SREDNJI: Nema Codex sesija u bazi
Codex sesije nisu importovane. Altevra ima `import` komandu ali samo za Hermes.

**Istraži:**
```bash
altevra import --help  # vidi koje toolove podržava
# Ako nema codex — treba custom import script
```

Custom import za Codex (session fajlovi su negde u `~/.codex/` ili Claude projects):
```bash
find /home/pavle/.claude/projects -name "*.jsonl" | head -20
# Altevra ima session recorder koji može da ingests custom JSONL
```

---

## 📊 DEEP TASK LIST za ovu sesiju

### Task A: Systemd service (30 min)
Implementiraj systemd service za brain daemon. Verifikuj da preživljava reboot.
```bash
systemctl --user status altevra-brain  # mora biti green
```

### Task B: Import sve sesije (1-2h)
Uvuci sve dostupne sesije iz svih sourcea:

```bash
# 1. Hermes (već urađeno, re-run je idempotent)
cd /home/pavle && altevra import --tool hermes

# 2. Istraži Claude Code transcript fajlove
find /home/pavle/.claude/projects -name "*.jsonl" -newer /tmp 2>/dev/null | wc -l
# Napiši custom import ako treba

# 3. Starije Claude sesije (home dir)
ls /home/pavle/.cache/claude-cli-nodejs/-home-pavle*/
# Svaki ima session transcripts

# 4. Provjeri koliko turns ima u bazi pre i posle
sqlite3 /home/pavle/.altevra/altevra.db "SELECT COUNT(*) FROM turns;"
```

### Task C: Observer konfiguracija (1h)
Observer ima 8 pattern detektora. Mora da radi i pravi insajte.

```bash
cd /home/pavle && altevra observer
cd /home/pavle && altevra brain status
# Vidi koje jobs su se izvršile
```

Ako nema insajta — istraži observer source kod:
```bash
find /home/pavle/projekti/ai-tooling/altevra -name "*.rs" | xargs grep -l "observer" | head -5
# Pročitaj logic i razumi šta traži
```

### Task D: Memory search verifikacija (30 min)
Altevra ima BM25 search po turns. Treba da vraća rezultate.

```bash
cd /home/pavle && altevra turn-search "ReVesta klijent prodaja"
cd /home/pavle && altevra recall "šta smo radili sa Hermes prošle nedelje"
cd /home/pavle && altevra memory search --query "voice gateway arhitektura"
```

Ako vraća 0 — vault indexer nije radio. Fix:
```bash
cd /home/pavle && altevra brain tick  # pokreni indexer ručno
```

### Task E: HTML Arhitektura dokument (2h)
Napravi BOLJI i DETALJNIJI HTML od onog koji postoji.

**Postojeći HTML:** `/home/pavle/Ideje/hermes-voice-gateway-arch.html`

Šta dodati u novi HTML:
1. **Altevra deep dive** — kako svaki job radi (vault_indexer, observer, classifier)
2. **Lokalni modeli sekcija** — xLAM, Qwen3, Whisper, Piper setup guide
3. **Import pipeline** — vizualizacija kako sesije iz Hermes/Claude/Codex ulaze
4. **Problem tracker** — live status svih poznatih problema sa fix-ovima
5. **Observer insights dashboard** — template za prikazivanje stvarnih insajta
6. **Interaktivni flow** — klikabilni dijagram, tooltip-i, expandable sections

**Novi fajl:** `/home/pavle/Ideje/altevra-deep-arch.html`

### Task F: Lokalni modeli setup (1-2h)
Pavle je radio na lokalnim modelima. Treba integrisati sa Altevra i Voice Gateway-em.

**Istraži šta je instalirano:**
```bash
# Ollama
ollama list 2>/dev/null || echo "ollama not installed"

# LM Studio
ls ~/LM-Studio* 2>/dev/null || ls ~/lm-studio* 2>/dev/null

# ContextLM
which contextlm 2>/dev/null || ls ~/projekti/ai-tooling/ | grep -i context

# Whisper lokalno
python3 -c "import whisper; print('whisper ok')" 2>/dev/null
python3 -c "from faster_whisper import WhisperModel; print('faster-whisper ok')" 2>/dev/null

# Piper TTS
which piper 2>/dev/null || find ~/projekti -name "piper" -type f 2>/dev/null | head -5
```

**Dokumentuj šta je dostupno i šta nedostaje za Voice Gateway.**

### Task G: Codex Rescue za implementaciju (kada zatreba)
Ako bilo koji od taskova zahteva pisanje koda (systemd script, import script, HTML), koristi:

```
/codex:rescue
```

Codex Rescue uzima kompleksne coding taskove i vraća working kod. Koristi ga za:
- Pisanje altevra-brain.service systemd fajla
- Custom import script za Claude Code JSONL sesije
- HTML dashboard sa stvarnim Altevra podacima
- Benchmark script za router modele

---

## 🗺️ ALTEVRA ARHITEKTURA — kako svaki deo funkcioniše

### Brain Daemon (`altevra brain start`)
Ticker koji svakih N sekundi pokreće jobs:
- `vault_indexer` (15min) — skenira vault fajlove, indeksira promene
- `observer_scan` (5min) — 8 pattern detektora nad turns bazom
- `classifier` — klasifikuje turns po tipu
- `embedder` — generiše embeddings za semantic search (ako je LLM konfigurisan)

### Observer (8 pattern detektora)
Detektuje u turns bazi:
1. **drift** — kad se fokus pomiče od P0 na side projects
2. **hook_failures** — kad hookovi ne rade
3. **stale_projects** — projekti bez aktivnosti dugo
4. **decision_conflicts** — kad nova odluka kontradiktuje staru
5. **secret_churn** — kad se API ključevi menjaju
6. **skill_divergence** — kad se skill promeni ali nije instaliran
7. **pattern_x, pattern_y** — custom detektori

### Hook Pipeline
```
Claude event → hook-handle → parse JSON → write to turns table → emit event
```

Svaki hook prima JSON na stdin:
```json
{
  "session_id": "abc123",
  "tool": "claude-code", 
  "event": "user_prompt_submit",
  "content": "...",
  "timestamp": "2026-06-07T14:00:00Z"
}
```

### Import Pipeline
```
~/.hermes/sessions/*.jsonl → altevra import --tool hermes → idempotent insert → turns table
```

Import je **idempotent** na `(tool, external_id)` — možeš re-run bez duplikata.

### MCP Server (`altevra serve --vault /home/pavle`)
Exposes toolove Claude Code-u:
- `search_turns` — BM25 search po content
- `recall_about` — temporal + entity recall
- `recall_window` — vremenski prozor sesija
- `get_observer_insights` — patterns iz observer job-ova
- `get_context_packet` — curated context za agente
- `get_last_updates` — nedavne promene
- `build_system_prompt` — kreira system prompt sa kontekstom
- `save_task`, `save_decision` — upiši u bazu

---

## 🔧 LOKALNI MODELI — Plan za Voice Gateway

### STT (Speech-to-Text)
**Cilj:** Whisper Turbo lokalno, streaming mode

```bash
# Install faster-whisper (preporučeno, 4x brže od openai/whisper)
pip install faster-whisper

# Test
python3 -c "
from faster_whisper import WhisperModel
model = WhisperModel('large-v3-turbo', device='cpu', compute_type='int8')
segments, info = model.transcribe('test.wav', language='sr')
print('Detected language:', info.language)
"
```

### Router LLM (mali model za dispatch)
**Cilj:** xLAM 1B ili Qwen3-0.6B via Ollama, JSON-only output

```bash
# Install Ollama
curl -fsSL https://ollama.com/install.sh | sh

# Pull modele za benchmark
ollama pull hf.co/Salesforce/xLAM-2-1b-fc-r-gguf  # primarni
ollama pull qwen3:0.6b                               # alternativa
ollama pull llama3.2:1b                              # fallback

# Test router call
curl -s http://localhost:11434/api/generate \
  -d '{"model":"qwen3:0.6b","prompt":"Route: git status","stream":false}' \
  | python3 -c "import json,sys; print(json.load(sys.stdin)['response'])"
```

### TTS (Text-to-Speech)
**Cilj:** Piper TTS, srpski glas, <500ms latency

```bash
# Install piper
pip install piper-tts

# Download srpski glas (ako postoji) ili engleski fallback
python3 -m piper --download-dir ~/.local/share/piper \
  --model en_US-amy-medium

# Test
echo "Primljeno. Šaljem Codexu." | python3 -m piper \
  --model ~/.local/share/piper/en_US-amy-medium.onnx \
  --output_file test.wav && aplay test.wav
```

### Benchmark Router Modele
**Pre nego što gradiš Voice Gateway, benchmark modele na srpskim komandama:**

Napravi `benchmark_router.py`:
```python
import json, time, requests

TEST_COMMANDS = [
    "git status",
    "pošalji Codexu da napravi bridge",
    "neka Claude Code analizira repo",
    "otvori terminal",
    "obriši sve node_modules foldere",
    "pokreni testove ali ne menjaj fajlove",
    "daj mi status Herdr workera",
    "pošalji poruku nekome",
    "pročitaj backend logove",
    "napravi GitHub issue za ovaj bug",
    # ... 90 više komandi
]

ROUTER_PROMPT = """You are Hermes Router. Return ONLY valid JSON.
Schema: {"route": "local_tool|herdr_codex|herdr_claude_code|gpt_mini|gpt55_high|block",
"task_type": "...", "thinking": "none|low|medium|high", "risk": "safe|confirm|block"}
Input: {input}"""

def test_model(model_name):
    results = []
    for cmd in TEST_COMMANDS:
        start = time.time()
        r = requests.post("http://localhost:11434/api/generate", json={
            "model": model_name,
            "prompt": ROUTER_PROMPT.format(input=cmd),
            "stream": False
        })
        latency = time.time() - start
        try:
            output = json.loads(r.json()['response'])
            valid_json = True
        except:
            valid_json = False
            output = {}
        results.append({
            "cmd": cmd, "latency": latency,
            "valid_json": valid_json, "route": output.get("route", "?")
        })
    return results
```

**Metrics koje meris:**
- Valid JSON % (mora biti >90%)
- Correct route % (ground truth ručno)
- Latency p50, p95
- False SAFE na dangerous commands (kritično)

---

## 📋 SESIJA CHECKLIST — Redosled rada

```
[ ] 1. Pročitaj ovaj fajl u celosti
[ ] 2. Proveri stanje baze: sqlite3 ~/.altevra/altevra.db "SELECT COUNT(*) FROM turns;"
[ ] 3. Proveri vault config: python3 -c "import json; d=json.load(open('/home/pavle/.claude.json')); print(d['mcpServers']['altevra']['args'])"
[ ] 4. Pokreni altevra brain status: cd /home/pavle && altevra brain status
[ ] 5. Task A: Napravi systemd service (kritično)
[ ] 6. Task B: Import sve sesije, vidi koliko turns ima posle
[ ] 7. Task C: Observer konfiguracija, vidi da li daje insajte
[ ] 8. Task D: Testiraj memory search sa pravim upitima
[ ] 9. Task F: Istraži lokalne modele (ollama, whisper, piper)
[ ] 10. Task E: Napravi bolji HTML dashboard
[ ] 11. Codex Rescue za bilo koji coding task >30 min
```

---

## 🚀 Codex Rescue — Kako koristiti

Kada treba da napišeš kod koji je >30 linija ili kompleksniji:

```
/codex:rescue
```

Daj Codex-u:
1. **Šta treba da napravi** — konkretan output (fajl, komanda, script)
2. **Šta zna** — lokacije binarnih, vault path, DB path
3. **Šta ne sme da dirne** — existing hooks, settings.json bez backupa
4. **Kako da testira** — konkretna komanda za verifikaciju

**Primeri task-ova za Codex:**
- "Napravi systemd service fajl za altevra brain koji se restartuje pri padu"
- "Napravi Python script koji importuje Claude Code JSONL transcripts u Altevra turns tabelu"
- "Napravi benchmark script za router modele sa 100 srpskih test komandi"
- "Napravi HTML dashboard koji čita direktno iz ~/.altevra/altevra.db i prikazuje live stats"

---

## 📁 Key Paths

| Šta | Gde |
|-----|-----|
| Altevra binary | `/home/pavle/projekti/ai-tooling/altevra/target/release/altevra` |
| Vault root | `/home/pavle` |
| DB | `/home/pavle/.altevra/altevra.db` |
| Config | `/home/pavle/.altevra/config.toml` |
| Brain log | `/home/pavle/.altevra/brain.log` |
| Claude settings | `/home/pavle/.claude/settings.json` |
| Claude global | `/home/pavle/.claude.json` |
| Hermes sessions | `/home/pavle/.hermes/sessions/*.jsonl` |
| Altevra source | `/home/pavle/projekti/ai-tooling/altevra/` |
| Obsidian vault | `/home/pavle/Obsidian/Imperium/` |
| HTML dokument | `/home/pavle/Ideje/hermes-voice-gateway-arch.html` |

---

## ⚡ Quick Commands

```bash
# Provjeri stanje
cd /home/pavle && altevra doctor

# Koliko turns ima
sqlite3 ~/.altevra/altevra.db "SELECT COUNT(*), MAX(created_at) FROM turns;"

# Sesije po toolovima
sqlite3 ~/.altevra/altevra.db \
  "SELECT tool, COUNT(*) FROM sessions GROUP BY tool;"

# Manual brain tick
cd /home/pavle && altevra brain tick

# Observer
cd /home/pavle && altevra observer

# Search turns
cd /home/pavle && altevra turn-search "ReVesta"

# Recall
cd /home/pavle && altevra recall "šta smo radili prošle nedelje"

# Import Hermes (idempotent)
cd /home/pavle && altevra import --tool hermes

# Rebuild binary ako ima kod promene
cd /home/pavle/projekti/ai-tooling/altevra && cargo build --release 2>&1 | tail -5
```

---

## 🎯 Definition of Done za ovu sesiju

Na kraju sesije mora biti true:
```
[ ] altevra brain radi kao systemd service, preživljava reboot
[ ] turns u bazi > 2000 (Hermes + Claude + šta god se nađe)
[ ] altevra observer vraća >0 insajta
[ ] altevra turn-search "ReVesta" vraća relevantne rezultate
[ ] altevra recall vraća koherentne odgovore
[ ] novi HTML dashboard sa live podacima iz baze
[ ] lokalni modeli dokumentovani (šta je instalirano, šta nedostaje)
[ ] MCP server u novoj Claude sesiji vraća podatke (ne 0)
```

---

*Briefing kreiran: 2026-06-07 | Prethodna sesija: claude-sonnet-4-6 | Imperium Tech*

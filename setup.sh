#!/usr/bin/env bash
#
# Altevra plug-and-play installer.  Clone the repo, then:
#
#     bash setup.sh
#
# Builds Altevra, puts it on PATH, connects your AI tools, configures the LLM
# (Claude via `claude -p` by default — uses your Claude subscription, no API key),
# and installs the autonomous background services. Idempotent: safe to re-run.
#
set -euo pipefail

say()  { printf '\n\033[1;31m▸ %s\033[0m\n' "$*"; }
ok()   { printf '  \033[32m✓\033[0m %s\n' "$*"; }
warn() { printf '  \033[33m!\033[0m %s\n' "$*"; }

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="${HOME}/.local/bin"
ALT="${BIN_DIR}/altevra"

# ── 1. build ──────────────────────────────────────────────────────────────────
say "Building Altevra (release, with local embedding)…"
if command -v cargo >/dev/null 2>&1; then
  ( cd "$REPO" && cargo build --release --features embedding )
  ok "built"
else
  echo "  cargo not found — install Rust first: https://rustup.rs" ; exit 1
fi

# ── 2. put on PATH ────────────────────────────────────────────────────────────
say "Installing binary to ${ALT}…"
mkdir -p "$BIN_DIR"
cp -f "$REPO/target/release/altevra" "$ALT"
case ":$PATH:" in
  *":$BIN_DIR:"*) ok "on PATH" ;;
  *) warn "add to your shell rc:  export PATH=\"\$HOME/.local/bin:\$PATH\"" ;;
esac

# ── 3. init ───────────────────────────────────────────────────────────────────
say "Initializing Altevra…"
"$ALT" init 2>/dev/null || true
ok "~/.altevra ready"

# ── 4. connect whatever AI tools are installed ────────────────────────────────
say "Connecting AI tools (auto-detect)…"
for tool in claude-code codex cursor hermes antigravity; do
  if "$ALT" connect --tool "$tool" >/dev/null 2>&1; then ok "connected $tool"; fi
done

# ── 5. LLM: Claude via claude -p (default, no API key) ────────────────────────
say "Configuring LLM…"
CFG="${HOME}/.altevra/config.toml"
if command -v claude >/dev/null 2>&1; then
  if ! grep -q 'kind = "claude-cli"' "$CFG" 2>/dev/null; then
    cat >> "$CFG" <<'TOML'

[llm]
reasoning_mode = "api"

[llm.cheap_worker]
kind  = "claude-cli"
model = "claude-haiku-4-5-20251001"

[llm.strong_reasoner]
kind  = "claude-cli"
model = "claude-sonnet-4-6"
TOML
  fi
  ok "Claude (claude -p) — Haiku=cheap, Sonnet=reasoning, no API key"
else
  warn "claude CLI not found — set an LLM later: 'altevra llm use codex|ollama|vllm'"
fi

# ── 6. background services (systemd user) ─────────────────────────────────────
say "Installing autonomous services…"
if command -v systemctl >/dev/null 2>&1; then
  "$ALT" service install --apply >/dev/null 2>&1 || true
  systemctl --user daemon-reload 2>/dev/null || true
  for s in altevra-brain altevra-embedder; do
    systemctl --user enable --now "${s}.service" >/dev/null 2>&1 || true
  done
  systemctl --user enable --now altevra-backup.timer >/dev/null 2>&1 || true
  loginctl enable-linger "$USER" >/dev/null 2>&1 || true
  ok "brain + embedder running (survive reboot)"
  warn "to capture ALL file work, also run:"
  echo "      altevra watch start --repo ~/projects --repo ~/Documents --repo ~/notes"
else
  warn "systemd not found — run the brain manually: 'altevra brain start'"
fi

say "Done. Altevra is live."
echo "  Try:  altevra recall \"what did I work on\"  ·  altevra brain status"

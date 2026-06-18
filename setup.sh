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

# ── 4. connect tools + LLM + services (native one-shot wizard) ────────────────
# Everything after the build is delegated to `altevra setup all`, the native
# wizard — one source of truth. Re-run `altevra setup all` any time to repair.
# Run from the repo so the skills source (06-skills/) is found regardless of
# where `bash setup.sh` was invoked from.
( cd "$REPO" && "$ALT" setup all )
# `altevra setup all` prints its own "Done. Altevra is live." closing banner —
# no second one here, or the user sees it twice.

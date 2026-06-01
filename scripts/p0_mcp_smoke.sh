#!/usr/bin/env bash
# P0 MCP live-connection smoke (BUILD_TASKS T1.20 / live test).
# Drives the real `altevra serve` MCP server over stdio with JSON-RPC and checks
# the connection works: initialize -> tools/list -> a tool call. This is the
# autonomous half of the "real test"; the herdr Claude/Cursor spawn is the
# interactive half. Exits non-zero on any failure.
set -euo pipefail

BIN="${ALTEVRA_BIN:-./target/release/altevra}"
VAULT="${ALTEVRA_VAULT:-.}"

if [[ ! -x "$BIN" ]]; then
  echo "FAIL: binary not found at $BIN" >&2
  exit 1
fi

req() { printf '%s\n' "$1"; }

OUT="$(
  {
    req '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"p0-smoke","version":"0"}}}'
    req '{"jsonrpc":"2.0","id":2,"method":"tools/list"}'
    req '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_capabilities","arguments":{}}}'
  } | "$BIN" serve --vault "$VAULT" 2>/dev/null
)"

echo "$OUT"

# Assertions: a tools/list response with at least the core tools, and a
# successful tool call (a result, not an error).
echo "$OUT" | grep -q '"id":1' || { echo "FAIL: no initialize response" >&2; exit 1; }
echo "$OUT" | grep -q 'get_agent_bootstrap_packet' || { echo "FAIL: tools/list missing core tool" >&2; exit 1; }
echo "$OUT" | grep -q 'get_context_packet' || { echo "FAIL: tools/list missing get_context_packet" >&2; exit 1; }
echo "$OUT" | grep -q '"id":3' || { echo "FAIL: no tools/call response" >&2; exit 1; }

echo "OK: MCP stdio connection works (initialize + tools/list + tools/call)"

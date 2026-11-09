#!/usr/bin/env bash
# End-to-end contract test for the OpenCode-compatible CLI and HTTP API.
# Usage: scripts/smoke.sh path/to/oxidecode
set -euo pipefail

BIN_ARG="${1:-target/release/oxidecode}"
if [ ! -x "$BIN_ARG" ]; then
  echo "binary not found: $BIN_ARG" >&2
  exit 1
fi
BIN="$(cd "$(dirname "$BIN_ARG")" && pwd)/$(basename "$BIN_ARG")"

ROOT="$(mktemp -d)"
PID=""
cleanup() {
  if [ -n "$PID" ]; then
    kill "$PID" 2>/dev/null || true
  fi
  "$BIN" service stop >/dev/null 2>&1 || true
  rm -rf "$ROOT"
}
trap cleanup EXIT

# Isolate every path so the test never touches a real installation.
export HOME="$ROOT/home"
export XDG_CONFIG_HOME="$ROOT/config"
export XDG_DATA_HOME="$ROOT/data"
export XDG_STATE_HOME="$ROOT/state"
export XDG_CACHE_HOME="$ROOT/cache"
export OXIDECODE_MOCK=1
mkdir -p "$HOME" "$ROOT/project"
printf '[workspace]\nmembers = []\n' > "$ROOT/project/Cargo.toml"
PROJECT="$ROOT/project"

pass() { printf '  ok  %s\n' "$1"; }
fail() { printf '  FAIL %s\n' "$1" >&2; exit 1; }

echo "== CLI contract =="
VERSION="$("$BIN" --version)"
case "$VERSION" in
  *2.2.6*) pass "--version ($VERSION)" ;;
  *) fail "--version expected 2.2.6, got: $VERSION" ;;
esac

"$BIN" models | grep -qx "mock/mock-echo" || fail "models must list mock/mock-echo"
pass "models"

if "$BIN" service status >"$ROOT/status.out" 2>/dev/null; then
  fail "service status must fail while stopped"
fi
[ ! -s "$ROOT/status.out" ] || fail "service status must not print while stopped"
pass "service status stopped"

URL="$("$BIN" service start)"
case "$URL" in
  http://127.0.0.1:*) pass "service start ($URL)" ;;
  *) fail "service start returned: $URL" ;;
esac
[ "$("$BIN" service status)" = "$URL" ] || fail "service status does not match start"
pass "service status running"

PASSWORD="$("$BIN" service get password)"
[ -n "$PASSWORD" ] || fail "empty service password"
pass "service get password"

PAIR="$("$BIN" pair)"
case "$PAIR" in
  "$URL"/auth/connect/*) pass "pair" ;;
  *) fail "pair returned: $PAIR" ;;
esac

echo "== HTTP API =="
api() {
  curl -sf -u "opencode:$PASSWORD" "$@"
}
json_field() {
  python3 -c 'import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1], {"d": d}))' "$1"
}
prompt() {
  local body
  body="$(python3 -c 'import json,sys; print(json.dumps({"text": sys.argv[1]}))' "$1")"
  api -H 'content-type: application/json' -d "$body" \
    "$URL/api/session/$SESSION/prompt" >/dev/null
}
wait_messages() {
  # $1: number of messages that must exist, $2: substring to find (optional)
  local target="$1" needle="${2:-}" count=0 i messages
  for i in $(seq 1 200); do
    messages="$(api "$URL/api/session/$SESSION/message?limit=5000&order=asc")"
    count="$(printf '%s' "$messages" | json_field 'len(d["data"])')"
    if [ "$count" -ge "$target" ]; then
      if [ -z "$needle" ] || printf '%s' "$messages" | grep -Fq "$needle"; then
        printf '%s' "$messages"
        return 0
      fi
    fi
    sleep 0.1
  done
  return 1
}

CREATE="$(api -H 'content-type: application/json' \
  -d "{\"title\":\"Smoke\",\"model\":{\"providerID\":\"mock\",\"id\":\"mock-echo\"},\"location\":{\"directory\":\"$PROJECT\"}}" \
  "$URL/api/session")"
SESSION="$(printf '%s' "$CREATE" | json_field 'd["data"]["id"]')"
case "$SESSION" in
  ses_*) pass "create session" ;;
  *) fail "invalid session id: $SESSION" ;;
esac

curl -sN -u "opencode:$PASSWORD" \
  "$URL/api/event?location%5Bdirectory%5D=$PROJECT" >"$ROOT/sse.log" &
SSE_PID=$!
sleep 0.4

BASE="$(api "$URL/api/session/$SESSION/message?limit=5000&order=asc" | json_field 'len(d["data"])')"
prompt "hello"
wait_messages "$((BASE + 3))" 'Echo: hello' >/dev/null || fail "echo prompt"
pass "prompt + echo response"

BASE="$(api "$URL/api/session/$SESSION/message?limit=5000&order=asc" | json_field 'len(d["data"])')"
prompt ':tool read {"filePath":"Cargo.toml"}'
wait_messages "$((BASE + 4))" 'Tool execution finished.' >/dev/null || fail "read tool"
pass "tool call (read)"

BASE="$(api "$URL/api/session/$SESSION/message?limit=5000&order=asc" | json_field 'len(d["data"])')"
prompt ':tool bash {"command":"echo smoke"}'
PERMISSION=""
for _ in $(seq 1 100); do
  PERMISSION="$(api "$URL/api/session/$SESSION/permission" | json_field 'd["data"][0]["id"] if d.get("data") else ""' 2>/dev/null || true)"
  [ -n "$PERMISSION" ] && break
  sleep 0.1
done
[ -n "$PERMISSION" ] || fail "permission request"
api -H 'content-type: application/json' -d '{"decision":"once"}' \
  "$URL/api/session/$SESSION/permission/$PERMISSION/reply" >/dev/null
pass "permission reply"
wait_messages "$((BASE + 4))" 'smoke' >/dev/null || fail "bash tool after permission"
pass "tool call (bash)"

sleep 0.5
kill "$SSE_PID" 2>/dev/null || true
PID=""
for type in session.text.delta session.text.ended session.tool.input.started \
            session.tool.called session.tool.success session.execution.succeeded; do
  grep -Fq "\"type\":\"$type\"" "$ROOT/sse.log" || fail "missing SSE event: $type"
done
pass "SSE frames"

echo "== lifecycle =="
"$BIN" service restart >/dev/null
[ "$("$BIN" service status)" != "" ] || fail "service restart"
pass "service restart"
"$BIN" service stop >/dev/null
if "$BIN" service status >/dev/null 2>&1; then
  fail "service status after stop"
fi
pass "service stop"

echo
echo "All smoke tests passed."

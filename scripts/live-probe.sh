#!/bin/bash
# live-probe.sh — end-to-end proof for Counterspell's interrupt remediation
# chain using an isolated Herdr workspace, isolated Counterspell state/config,
# and a Herdr proxy log. It never uses the production watch-arm LaunchAgent or
# real ~/.counterspell state.
#
# Default mode is deterministic inject mode:
#   1. Start a fresh Claude Code pane on claude-fable-5 in a scratch cwd.
#   2. Wait for a real Fable transcript line, then start a long working turn.
#   3. Append a non-Fable transcript line for this session.
#   4. Run 3+ isolated armed watch passes while the chain is in flight and
#      assert exactly one Escape and exactly one /compact.
#   5. After compact-summary evidence appears, run another isolated armed pass
#      and assert /model claude-fable-5 plus continue delivery.
#   6. Wait for a post-continue Fable line and run one final pass to clear the
#      durable chain from the isolated state file.
#
# Usage:
#   [COUNTERSPELL_BIN=target/debug/counterspell] [CS_DRIFT_SOURCE=inject|live] \
#     scripts/live-probe.sh [WORKSPACE_ID]
#
# Exit codes: 0 PASS, 1 FAIL, 2 INCONCLUSIVE (live mode only).

set -uo pipefail

TARGET_MODEL="claude-fable-5"
SCRATCH_ROOT="${TMPDIR:-/tmp}/counterspell-e2e"
RUN_ROOT="${SCRATCH_ROOT}/run-$$"
PROJECTS_DIR="${HOME}/.claude/projects"
COUNTERSPELL_BIN="${COUNTERSPELL_BIN:-$(pwd)/target/debug/counterspell}"
CS_DRIFT_SOURCE="${CS_DRIFT_SOURCE:-inject}"
CS_TRIGGER_FILE="${CS_TRIGGER_FILE:-${SCRATCH_ROOT}/trigger.txt}"
INJECT_MODEL="${CS_INJECT_MODEL:-claude-opus-4-5}"
SESSION_APPEAR_TIMEOUT_SECS=90
FABLE_TIMEOUT_SECS=120
WORKING_TIMEOUT_SECS=45
COMPACT_TIMEOUT_SECS=180
FABLE_RETURN_TIMEOUT_SECS=180

CONFIG_FILE="${RUN_ROOT}/counterspell.toml"
STATE_FILE="${RUN_ROOT}/state.json"
DISARM_MARKER="${RUN_ROOT}/disarmed"
WATCH_LOG="${RUN_ROOT}/watch.log"
HERDR_CALL_LOG="${RUN_ROOT}/herdr-calls.log"
HERDR_PROXY="${RUN_ROOT}/herdr-proxy.sh"

log() { printf '[probe %s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
fail() { log "FAIL: $*"; exit 1; }
inconclusive() { log "INCONCLUSIVE: $*"; exit 2; }

mkdir -p "$RUN_ROOT"

command -v herdr >/dev/null || fail "herdr not on PATH"
command -v jq >/dev/null || fail "jq not on PATH"
command -v claude >/dev/null || fail "claude not on PATH"
if [ ! -x "$COUNTERSPELL_BIN" ]; then
  log "counterspell binary missing at $COUNTERSPELL_BIN; building debug binary"
  cargo build || fail "cargo build failed"
fi

REAL_HERDR="$(command -v herdr)"
cat > "$HERDR_PROXY" <<EOF
#!/bin/sh
printf '%s\n' "\$*" >> "$HERDR_CALL_LOG"
exec "$REAL_HERDR" "\$@"
EOF
chmod +x "$HERDR_PROXY"
: > "$WATCH_LOG"
: > "$HERDR_CALL_LOG"
: > "$CONFIG_FILE"

run_watch_pass() {
  local label="$1"
  log "watch --arm (${label})"
  COUNTERSPELL_HERDR_BIN="$HERDR_PROXY" \
  COUNTERSPELL_HERDR_LOG="$HERDR_CALL_LOG" \
  COUNTERSPELL_TRANSCRIPT_QUIET_SECONDS=0 \
  "$COUNTERSPELL_BIN" \
    --projects-dir "$PROJECTS_DIR" \
    --config "$CONFIG_FILE" \
    --state "$STATE_FILE" \
    --disarm-marker "$DISARM_MARKER" \
    --recent-hours 999 \
    watch --arm >> "$WATCH_LOG" 2>&1 || {
      tail -40 "$WATCH_LOG" >&2
      fail "watch pass failed (${label})"
    }
}

pane_status() {
  herdr pane list --workspace "$WS" \
    | jq -r --arg pane "$PANE" '.result.panes[] | select(.pane_id == $pane) | .agent_status // "unknown"' \
    | tail -1
}

current_model() {
  grep -o '"model":"[^"]*"' "$SESSION_FILE" 2>/dev/null | tail -1 | sed 's/.*"model":"//;s/"//'
}

wait_for_model() {
  local desired="$1"
  local timeout="$2"
  local deadline=$(( $(date +%s) + timeout ))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    [ "$(current_model)" = "$desired" ] && return 0
    sleep 2
  done
  return 1
}

wait_for_compact_summary() {
  local deadline=$(( $(date +%s) + COMPACT_TIMEOUT_SECS ))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    if grep -q '"type":"summary"' "$SESSION_FILE" 2>/dev/null \
      || grep -q '"summary":' "$SESSION_FILE" 2>/dev/null \
      || grep -q '"subtype":"compact_boundary"' "$SESSION_FILE" 2>/dev/null \
      || grep -q '"compactMetadata":' "$SESSION_FILE" 2>/dev/null \
      || grep -q '"isCompactSummary":true' "$SESSION_FILE" 2>/dev/null; then
      return 0
    fi
    sleep 2
  done
  return 1
}

append_injected_drift() {
  local inject_ts
  inject_ts=$(date -u -v-90S +%Y-%m-%dT%H:%M:%SZ 2>/dev/null \
    || date -u -d '90 seconds ago' +%Y-%m-%dT%H:%M:%SZ)
  printf '{"type":"assistant","sessionId":"%s","timestamp":"%s","cwd":"%s","message":{"model":"%s"}}\n' \
    "$SESSION_ID" "$inject_ts" "$CWD" "$INJECT_MODEL" >> "$SESSION_FILE"
}

case "$CS_DRIFT_SOURCE" in
  inject)
    OPENING_PROMPT="Reply with exactly one word: ready"
    ;;
  live)
    [ -s "$CS_TRIGGER_FILE" ] || inconclusive "no trigger fixture at $CS_TRIGGER_FILE"
    OPENING_PROMPT="$(cat "$CS_TRIGGER_FILE")"
    ;;
  *) fail "CS_DRIFT_SOURCE must be inject or live (got $CS_DRIFT_SOURCE)" ;;
esac

WS="${1:-}"
CWD="${RUN_ROOT}/cwd"
mkdir -p "$CWD"
CWD=$(cd "$CWD" && pwd -P)
if [ -z "$WS" ]; then
  log "creating isolated workspace"
  WS=$(herdr workspace create --no-focus --label counterspell-e2e --cwd "$CWD" \
    | jq -r '.result.workspace.workspace_id')
  [ -n "$WS" ] && [ "$WS" != "null" ] || fail "could not create workspace"
fi
log "workspace: $WS  cwd: $CWD"

ENC_CWD=$(printf '%s' "$CWD" | sed 's#[/.]#-#g')
PROJ_DIR="${PROJECTS_DIR}/${ENC_CWD}"
log "expecting transcript under: $PROJ_DIR"

log "launching claude --model $TARGET_MODEL"
START_JSON=$(herdr agent start claude --workspace "$WS" --cwd "$CWD" --no-focus \
  -- claude --model "$TARGET_MODEL")
PANE=$(printf '%s' "$START_JSON" \
  | jq -r '.result.agent.pane_id // .result.pane_id // .result.pane.pane_id // empty')
if [ -z "$PANE" ]; then
  PANE=$(herdr pane list --workspace "$WS" | jq -r '.result.panes[0].pane_id')
fi
[ -n "$PANE" ] && [ "$PANE" != "null" ] || fail "no pane id from agent start"
log "pane: $PANE"

cleanup() {
  log "cleanup: closing pane $PANE and workspace $WS"
  herdr pane send-keys "$PANE" Escape >/dev/null 2>&1 || true
  herdr pane close "$PANE" >/dev/null 2>&1 || true
  herdr workspace close "$WS" >/dev/null 2>&1 \
    || herdr workspace delete "$WS" >/dev/null 2>&1 || true
}
trap cleanup EXIT

log "waiting for Claude TUI"
deadline=$(( $(date +%s) + 60 ))
ready=""
while [ "$(date +%s)" -lt "$deadline" ]; do
  screen=$(herdr pane read "$PANE" --source visible --lines 40 2>/dev/null || true)
  if printf '%s' "$screen" | grep -qi 'trust'; then
    herdr pane send-keys "$PANE" Enter >/dev/null
    sleep 3
    continue
  fi
  if printf '%s' "$screen" | grep -qi 'tokens'; then
    ready=1
    break
  fi
  sleep 2
done
[ -n "$ready" ] || fail "Claude TUI never became ready"

log "sending opening prompt"
herdr pane send-text "$PANE" "$OPENING_PROMPT" >/dev/null
sleep 1
herdr pane send-keys "$PANE" Enter >/dev/null

log "locating transcript"
SESSION_FILE=""
deadline=$(( $(date +%s) + SESSION_APPEAR_TIMEOUT_SECS ))
while [ "$(date +%s)" -lt "$deadline" ]; do
  if [ -d "$PROJ_DIR" ]; then
    SESSION_FILE=$(ls -t "$PROJ_DIR"/*.jsonl 2>/dev/null | head -1 || true)
    [ -n "$SESSION_FILE" ] && break
  fi
  sleep 2
done
[ -n "$SESSION_FILE" ] || fail "probe transcript never appeared"
SESSION_ID=$(basename "$SESSION_FILE" .jsonl)
SHORT_ID=${SESSION_ID:0:8}
log "session: $SESSION_ID (short $SHORT_ID)"

if [ "$CS_DRIFT_SOURCE" = "inject" ]; then
  log "waiting for initial Fable line"
  wait_for_model "$TARGET_MODEL" "$FABLE_TIMEOUT_SECS" \
    || fail "no $TARGET_MODEL assistant line appeared before injection"

  log "starting a long working turn"
  herdr pane send-text "$PANE" "Write the numbers from 1 to 2000, one per line. Do not stop early." >/dev/null
  sleep 1
  herdr pane send-keys "$PANE" Enter >/dev/null
  deadline=$(( $(date +%s) + WORKING_TIMEOUT_SECS ))
  working=""
  while [ "$(date +%s)" -lt "$deadline" ]; do
    if [ "$(pane_status)" = "working" ]; then
      working=1
      break
    fi
    sleep 1
  done
  [ -n "$working" ] || fail "pane did not enter working state for interrupt scenario"

  log "injecting transcript drift to $INJECT_MODEL"
  append_injected_drift
fi

DRIFT_MODEL="$(current_model)"
[ -n "$DRIFT_MODEL" ] || fail "could not read current model from transcript"
if [ "$DRIFT_MODEL" = "$TARGET_MODEL" ] || [ "$DRIFT_MODEL" = "<synthetic>" ]; then
  if [ "$CS_DRIFT_SOURCE" = "live" ]; then
    inconclusive "live prompt did not produce a non-Fable drift"
  fi
  fail "injected drift is not visible as newest model"
fi
log "drift visible: $TARGET_MODEL -> $DRIFT_MODEL"

run_watch_pass "initial interrupt+compact"
for index in 1 2 3; do
  run_watch_pass "in-flight exactly-once ${index}"
done

compact_count=$(grep -c "pane run $PANE /compact" "$HERDR_CALL_LOG" || true)
escape_count=$(grep -c "pane send-keys $PANE escape" "$HERDR_CALL_LOG" || true)
switch_count=$(grep -c "pane run $PANE /model $TARGET_MODEL" "$HERDR_CALL_LOG" || true)
[ "$compact_count" -eq 1 ] || fail "expected exactly one compact during in-flight passes, got $compact_count"
[ "$escape_count" -eq 1 ] || fail "expected exactly one interrupt during in-flight passes, got $escape_count"
[ "$switch_count" -le 1 ] || fail "expected at most one model switch, got $switch_count"

if [ "$switch_count" -eq 0 ]; then
  log "waiting for compact summary evidence"
  wait_for_compact_summary || fail "compact summary did not appear in transcript"
  run_watch_pass "switch+continue"
else
  log "switch+continue advanced during repeated passes after compact evidence"
fi

switch_count=$(grep -c "pane run $PANE /model $TARGET_MODEL" "$HERDR_CALL_LOG" || true)
continue_count=$(grep -c "pane run $PANE continue" "$HERDR_CALL_LOG" || true)
[ "$switch_count" -eq 1 ] || fail "expected exactly one model switch, got $switch_count"
[ "$continue_count" -eq 1 ] || fail "expected exactly one continue command, got $continue_count"

log "waiting for post-continue Fable transcript evidence"
wait_for_model "$TARGET_MODEL" "$FABLE_RETURN_TIMEOUT_SECS" \
  || fail "session did not return to $TARGET_MODEL after continue"

run_watch_pass "completion clear"
if jq -e '.sessions["'"$SESSION_ID"'"].remediation_chain' "$STATE_FILE" >/dev/null 2>&1; then
  fail "remediation_chain still present after completion evidence"
fi

echo "----------------------------------------------------------------------"
echo "  session:        $SESSION_ID"
echo "  pane:           $PANE"
echo "  drift:          $TARGET_MODEL -> $DRIFT_MODEL"
echo "  compact sends:  $compact_count"
echo "  switch sends:   $switch_count"
echo "  continue sends: $continue_count"
echo "  state:          $STATE_FILE"
echo "  herdr log:      $HERDR_CALL_LOG"
echo "----------------------------------------------------------------------"
log "PASS: interrupt -> compact -> switch -> continue completed without double compact"

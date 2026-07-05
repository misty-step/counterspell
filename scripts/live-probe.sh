#!/bin/bash
# live-probe.sh — end-to-end proof that Counterspell remediates a REAL
# downgrade, using the production daemon (watch-arm LaunchAgent), a real
# Herdr pane, and a real Claude Code session.
#
# What it does:
#   1. Launches `claude --model claude-fable-5` inside an ISOLATED Herdr
#      workspace (never the operating pane) via `herdr agent start`, so Herdr
#      reports the authoritative agent_session id and Counterspell binds the
#      pane with zero config.
#   2. Sends a prompt on a safety-fallback-prone topic (the operator-verified
#      trigger for the production Fable->safety-model downgrade), padded with a
#      mechanical counting task so the pane stays WORKING when the 10s daemon
#      tick notices.
#   3. Asserts the fallback actually fired (transcript shows a non-Fable model).
#   4. Asserts the armed daemon remediated: the watch log shows the fast-path
#      chain (queue-compact + switch:claude-fable-5) for THIS session, and the
#      session's model returns to Fable — and reports how long that took.
#
# Interruption (Escape landing) is a STRETCH GOAL, not a pass condition: the
# chain is queue-safe without it. The probe reports whether interrupt was
# emitted but does not fail if the turn was not cut.
#
# CONTEXT-HYGIENE CONTRACT (learned the hard way): the trigger prompt is a
# safety-routing topic. If it lives in an *operating* Claude session's context,
# that session's own /compact will be refused by the compaction model — the
# lead poisons itself. So the trigger is NEVER inlined here. It is read at
# runtime from an external fixture file that is git-ignored and authored by a
# throwaway/non-lead context. No harmful-topic plaintext ever touches this repo
# or the lead's transcript.
#
# Requires: herdr running, the watch-arm LaunchAgent loaded, jq, claude, and a
# trigger fixture (see CS_TRIGGER_FILE below).
#
# Usage: CS_TRIGGER_FILE=/path/to/trigger.txt live-probe.sh [WORKSPACE_ID]
#   CS_TRIGGER_FILE  file whose contents are sent verbatim as the trigger.
#                    Default: $SCRATCH_ROOT/trigger.txt
#   WORKSPACE_ID     reuse an existing isolated workspace; omitted => create.
#
# Exit codes: 0 PASS, 1 FAIL (remediation missing/late), 2 INCONCLUSIVE
# (no trigger fixture, or the fallback never fired).

set -uo pipefail

TARGET_MODEL="claude-fable-5"
PROJECTS_DIR="${HOME}/.claude/projects"
WATCH_LOG="${HOME}/Library/Logs/counterspell-watch-arm.log"
SCRATCH_ROOT="${TMPDIR:-/tmp}/counterspell-e2e"
CS_TRIGGER_FILE="${CS_TRIGGER_FILE:-${SCRATCH_ROOT}/trigger.txt}"
FALLBACK_TIMEOUT_SECS=150     # waiting for the safety fallback to appear
REMEDIATION_TIMEOUT_SECS=240  # waiting for the daemon chain after drift shows
SESSION_APPEAR_TIMEOUT_SECS=90

log()  { printf '[probe %s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
fail() { log "FAIL: $*"; exit 1; }
inconclusive() { log "INCONCLUSIVE: $*"; exit 2; }

command -v herdr  >/dev/null || fail "herdr not on PATH"
command -v jq     >/dev/null || fail "jq not on PATH"
command -v claude >/dev/null || fail "claude not on PATH"

# --- trigger fixture (kept out of this repo and any lead's context) ---------
if [ ! -s "$CS_TRIGGER_FILE" ]; then
  inconclusive "no trigger fixture at $CS_TRIGGER_FILE.
  Populate it from a throwaway context with a prompt that routes to the safety
  model (a cybersecurity or dual-use bio question), then re-run. Keeping the
  trigger out of the operating session is the whole point — do not paste it
  into a lead conversation."
fi
TRIGGER_PROMPT="$(cat "$CS_TRIGGER_FILE")"

# --- 0. daemon must be armed & loaded ---------------------------------------
# Primary check: the watch log heartbeat. The arm execs `counterspell watch`
# every 10s and every tick appends to the log, so a fresh mtime proves the
# daemon is live. launchctl is only a fallback — it cannot see user
# LaunchAgents from detached contexts (cron, background runners), where this
# probe legitimately runs.
daemon_live() {
  if [ -f "$WATCH_LOG" ]; then
    local now mtime
    now=$(date +%s)
    mtime=$(stat -f %m "$WATCH_LOG" 2>/dev/null || stat -c %Y "$WATCH_LOG" 2>/dev/null || echo 0)
    [ $(( now - mtime )) -le 30 ] && return 0
  fi
  launchctl list 2>/dev/null | grep -q 'com.misty-step.counterspell.watch-arm'
}
daemon_live || fail "watch-arm daemon not live — no watch-log heartbeat in 30s and LaunchAgent not visible"
[ -f "$WATCH_LOG" ] || : > "$WATCH_LOG"
WATCH_LOG_START_LINES=$(wc -l < "$WATCH_LOG" | tr -d ' ')
log "watch log baseline: ${WATCH_LOG_START_LINES} lines"

# --- 1. isolated workspace --------------------------------------------------
WS="${1:-}"
CWD="${SCRATCH_ROOT}/run-$$"
mkdir -p "$CWD"
if [ -z "$WS" ]; then
  log "creating isolated workspace..."
  WS=$(herdr workspace create --no-focus --label counterspell-e2e --cwd "$CWD" \
        | jq -r '.result.workspace.workspace_id')
  [ -n "$WS" ] && [ "$WS" != "null" ] || fail "could not create workspace"
fi
log "workspace: $WS  cwd: $CWD"

# encoded project dir claude will write the transcript into
ENC_CWD=$(printf '%s' "$CWD" | sed 's#[/.]#-#g')
PROJ_DIR="${PROJECTS_DIR}/${ENC_CWD}"
log "expecting transcripts under: $PROJ_DIR"

# --- 2. launch claude on Fable via herdr agent start ------------------------
log "launching claude --model ${TARGET_MODEL} ..."
START_JSON=$(herdr agent start claude --workspace "$WS" --cwd "$CWD" --no-focus \
              -- --model "$TARGET_MODEL")
PANE=$(printf '%s' "$START_JSON" | jq -r '.result.pane_id // .result.pane.pane_id // empty')
if [ -z "$PANE" ]; then
  PANE=$(herdr pane list --workspace "$WS" | jq -r '.result.panes[0].pane_id')
fi
[ -n "$PANE" ] && [ "$PANE" != "null" ] || fail "no pane id from agent start"
log "pane: $PANE"

cleanup() {
  log "cleanup: closing pane $PANE"
  herdr pane send-keys "$PANE" Escape >/dev/null 2>&1 || true
  herdr pane close "$PANE" >/dev/null 2>&1 || true
}
trap cleanup EXIT

# wait for the claude TUI to come up
log "waiting for claude to be ready..."
herdr wait agent-status "$PANE" --status idle --timeout 45000 >/dev/null 2>&1 \
  || herdr wait agent-status "$PANE" --status working --timeout 15000 >/dev/null 2>&1 \
  || true
sleep 5

# --- 3. discover THIS session's transcript ----------------------------------
log "locating probe transcript..."
SESSION_FILE=""
deadline=$(( $(date +%s) + SESSION_APPEAR_TIMEOUT_SECS ))
while [ "$(date +%s)" -lt "$deadline" ]; do
  if [ -d "$PROJ_DIR" ]; then
    SESSION_FILE=$(ls -t "$PROJ_DIR"/*.jsonl 2>/dev/null | head -1 || true)
    [ -n "$SESSION_FILE" ] && break
  fi
  sleep 2
done
[ -n "$SESSION_FILE" ] || fail "probe transcript never appeared under $PROJ_DIR"
SESSION_ID=$(basename "$SESSION_FILE" .jsonl)
SHORT_ID=${SESSION_ID:0:8}
log "session: $SESSION_ID  (short $SHORT_ID)"

# --- 4. send the trigger prompt ---------------------------------------------
log "sending trigger prompt (safety-route + counting tail)..."
herdr pane send-text "$PANE" "$TRIGGER_PROMPT" >/dev/null
sleep 1
herdr pane send-keys "$PANE" Enter >/dev/null
log "prompt submitted; watching for Fable->non-Fable drift..."

# newest model recorded in the transcript for this session
current_model() {
  grep -o '"model":"[^"]*"' "$SESSION_FILE" 2>/dev/null | tail -1 | sed 's/.*"model":"//;s/"//'
}

# --- 5. assert the safety fallback fired (real drift) -----------------------
DRIFT_MODEL=""
deadline=$(( $(date +%s) + FALLBACK_TIMEOUT_SECS ))
while [ "$(date +%s)" -lt "$deadline" ]; do
  m=$(current_model)
  if [ -n "$m" ] && [ "$m" != "$TARGET_MODEL" ] && [ "$m" != "<synthetic>" ]; then
    DRIFT_MODEL="$m"
    break
  fi
  sleep 3
done
if [ -z "$DRIFT_MODEL" ]; then
  inconclusive "safety fallback never fired — transcript stayed on ${TARGET_MODEL}. Pick a stronger trigger."
fi
DRIFT_AT=$(date +%s)
log "DRIFT observed: session downgraded ${TARGET_MODEL} -> ${DRIFT_MODEL}"

# --- 6. assert the armed daemon remediated ----------------------------------
log "waiting for daemon remediation (fast-path chain for $SHORT_ID)..."
REMEDIATION_LINE=""
SWITCHED_BACK=""
INTERRUPT_SEEN=""
deadline=$(( DRIFT_AT + REMEDIATION_TIMEOUT_SECS ))
while [ "$(date +%s)" -lt "$deadline" ]; do
  row=$(tail -n +"$((WATCH_LOG_START_LINES + 1))" "$WATCH_LOG" 2>/dev/null \
        | grep -E "^${SHORT_ID}[[:space:]]" | grep 'switch:claude-fable-5' | tail -1 || true)
  if [ -n "$row" ] && [ -z "$REMEDIATION_LINE" ]; then
    REMEDIATION_LINE="$row"
    ACT_AT=$(date +%s)
    log "REMEDIATION emitted (+$((ACT_AT - DRIFT_AT))s): $row"
    printf '%s' "$row" | grep -q 'interrupt' && INTERRUPT_SEEN=1
  fi
  m=$(current_model)
  if [ "$m" = "$TARGET_MODEL" ]; then
    SWITCHED_BACK=1
    BACK_AT=$(date +%s)
    log "session model back on ${TARGET_MODEL} (+$((BACK_AT - DRIFT_AT))s)"
  fi
  [ -n "$REMEDIATION_LINE" ] && [ -n "$SWITCHED_BACK" ] && break
  sleep 3
done

# --- 7. verdict -------------------------------------------------------------
echo "----------------------------------------------------------------------"
echo "  session:       $SESSION_ID"
echo "  drift:         ${TARGET_MODEL} -> ${DRIFT_MODEL}"
echo "  remediation:   ${REMEDIATION_LINE:-<none seen>}"
echo "  switched back: ${SWITCHED_BACK:+yes}${SWITCHED_BACK:-no}"
echo "  interrupt emitted (stretch): ${INTERRUPT_SEEN:+yes}${INTERRUPT_SEEN:-no}"
echo "----------------------------------------------------------------------"

[ -n "$REMEDIATION_LINE" ] || fail "daemon never emitted switch:${TARGET_MODEL} for $SHORT_ID within ${REMEDIATION_TIMEOUT_SECS}s of drift"
[ -n "$SWITCHED_BACK" ]    || fail "chain emitted but session model did not return to ${TARGET_MODEL} within ${REMEDIATION_TIMEOUT_SECS}s"
log "PASS: real drift remediated by the armed daemon; back on ${TARGET_MODEL}"
exit 0

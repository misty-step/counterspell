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
# DRIFT SOURCE: two modes, CS_DRIFT_SOURCE=inject (default) | live.
#   inject — deterministic: after one benign Fable exchange proves the session
#            is bound and on-target, append a realistic non-Fable assistant
#            line (backdated past the transcript-quiet gate) to THIS session's
#            transcript. Exercises every daemon path — watch, parse, bind,
#            gate, fast-path chain — except the upstream safety-routing event
#            itself. This is the CI-able mode.
#   live   — the original full-stack mode: send a safety-fallback-prone trigger
#            prompt and wait for a REAL routing downgrade. Nondeterministic
#            (routing may simply not fire); needs the external fixture.
#
# Requires: herdr running, the watch-arm LaunchAgent loaded, jq, claude, and
# (live mode only) a trigger fixture (see CS_TRIGGER_FILE below).
#
# Usage: [CS_DRIFT_SOURCE=inject|live] live-probe.sh [WORKSPACE_ID]
#   CS_TRIGGER_FILE  (live mode) file sent verbatim as the trigger.
#                    Default: $SCRATCH_ROOT/trigger.txt
#   WORKSPACE_ID     reuse an existing isolated workspace; omitted => create.
#
# Exit codes: 0 PASS, 1 FAIL (remediation missing/late), 2 INCONCLUSIVE
# (live mode: no trigger fixture, or the fallback never fired).

set -uo pipefail

TARGET_MODEL="claude-fable-5"
PROJECTS_DIR="${HOME}/.claude/projects"
WATCH_LOG="${HOME}/Library/Logs/counterspell-watch-arm.log"
SCRATCH_ROOT="${TMPDIR:-/tmp}/counterspell-e2e"
CS_TRIGGER_FILE="${CS_TRIGGER_FILE:-${SCRATCH_ROOT}/trigger.txt}"
CS_DRIFT_SOURCE="${CS_DRIFT_SOURCE:-inject}"
# inject mode: a realistic non-Fable, non-sentinel model string. What the
# daemon reacts to is "latest real model != target"; the exact name only needs
# to be plausible.
INJECT_MODEL="${CS_INJECT_MODEL:-claude-opus-4-5}"
FALLBACK_TIMEOUT_SECS=150     # waiting for the safety fallback to appear
REMEDIATION_TIMEOUT_SECS=240  # waiting for the daemon chain after drift shows
SESSION_APPEAR_TIMEOUT_SECS=90

log()  { printf '[probe %s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
fail() { log "FAIL: $*"; exit 1; }
inconclusive() { log "INCONCLUSIVE: $*"; exit 2; }

command -v herdr  >/dev/null || fail "herdr not on PATH"
command -v jq     >/dev/null || fail "jq not on PATH"
command -v claude >/dev/null || fail "claude not on PATH"

# --- prompt selection per drift source ---------------------------------------
case "$CS_DRIFT_SOURCE" in
  inject)
    # Benign by design: the exchange exists only to (a) create the transcript
    # and (b) put one REAL claude-fable-5 assistant line in model_history,
    # which the daemon's auto-Fable targeting requires before it will act.
    TRIGGER_PROMPT="Reply with exactly one word: ready"
    ;;
  live)
    # Trigger fixture kept out of this repo and any lead's context: the prompt
    # is a safety-routing topic, and if it lives in an operating session's
    # context that session's own /compact gets refused (see header contract).
    if [ ! -s "$CS_TRIGGER_FILE" ]; then
      inconclusive "no trigger fixture at $CS_TRIGGER_FILE.
      Populate it from a throwaway context with a prompt that routes to the
      safety model, then re-run. Keeping the trigger out of the operating
      session is the whole point — do not paste it into a lead conversation."
    fi
    TRIGGER_PROMPT="$(cat "$CS_TRIGGER_FILE")"
    ;;
  *) fail "CS_DRIFT_SOURCE must be 'inject' or 'live' (got '$CS_DRIFT_SOURCE')" ;;
esac

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
# Claude Code names the project dir after the RESOLVED cwd (symlinks like
# /var -> /private/var expanded, no duplicate slashes) — resolve ours to match.
CWD=$(cd "$CWD" && pwd -P)
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
# NOTE: argv after `--` is the FULL command line (herdr replaces the agent's
# default command), so the binary name must be repeated.
START_JSON=$(herdr agent start claude --workspace "$WS" --cwd "$CWD" --no-focus \
              -- claude --model "$TARGET_MODEL")
PANE=$(printf '%s' "$START_JSON" | jq -r '.result.agent.pane_id // .result.pane_id // .result.pane.pane_id // empty')
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

# Wait for the claude TUI by reading actual pane content — `herdr wait
# agent-status` returns instantly for a pane with no reported status, so it is
# useless as a readiness gate. Two screens matter:
#   - the folder-trust dialog (fresh scratch cwds always trigger it): the
#     trusting option is preselected, one Enter clears it;
#   - the real input screen, discriminated by its token-count footer, which
#     the trust dialog lacks.
log "waiting for claude TUI (accepting trust dialog if shown)..."
TUI_READY=""
deadline=$(( $(date +%s) + 60 ))
while [ "$(date +%s)" -lt "$deadline" ]; do
  # The pane can be very narrow (~24 cols), so screen text hard-wraps mid
  # phrase — match single words that survive wrapping, never multi-word spans.
  screen=$(herdr pane read "$PANE" --source visible --lines 40 2>/dev/null || true)
  if printf '%s' "$screen" | grep -qi 'trust'; then
    log "accepting folder-trust dialog..."
    herdr pane send-keys "$PANE" Enter >/dev/null
    sleep 3
    continue
  fi
  if printf '%s' "$screen" | grep -qi 'tokens'; then
    TUI_READY=1
    break
  fi
  sleep 2
done
[ -n "$TUI_READY" ] || fail "claude TUI never became ready in pane $PANE"
sleep 2

# --- 3. send the trigger prompt ---------------------------------------------
# This happens BEFORE transcript discovery: Claude Code creates the session
# .jsonl lazily on the first exchange, so waiting for it pre-prompt deadlocks.
log "sending ${CS_DRIFT_SOURCE}-mode opening prompt..."
herdr pane send-text "$PANE" "$TRIGGER_PROMPT" >/dev/null
sleep 1
herdr pane send-keys "$PANE" Enter >/dev/null
log "prompt submitted"

# --- 4. discover THIS session's transcript ----------------------------------
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

# newest model recorded in the transcript for this session
current_model() {
  grep -o '"model":"[^"]*"' "$SESSION_FILE" 2>/dev/null | tail -1 | sed 's/.*"model":"//;s/"//'
}

# --- 5a. (inject mode) manufacture the drift ---------------------------------
if [ "$CS_DRIFT_SOURCE" = "inject" ]; then
  # The daemon's auto-Fable targeting requires a real claude-fable-5 entry in
  # model_history before it will watch the session — wait for the opening
  # exchange's assistant line first.
  log "waiting for a real ${TARGET_MODEL} assistant line before injecting..."
  fable_seen=""
  deadline=$(( $(date +%s) + 90 ))
  while [ "$(date +%s)" -lt "$deadline" ]; do
    if grep -q "\"model\":\"${TARGET_MODEL}\"" "$SESSION_FILE" 2>/dev/null; then
      fable_seen=1
      break
    fi
    sleep 2
  done
  [ -n "$fable_seen" ] || fail "no ${TARGET_MODEL} assistant line appeared in $SESSION_FILE within 90s"
  # Backdate past the transcript-quiet gate (default 30s) so the daemon acts
  # on its next 10s tick instead of waiting out the quiet window. The daemon
  # takes last_event_at from the final line's timestamp, so this both creates
  # the drift and marks the transcript quiet.
  inject_ts=$(date -u -v-90S +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || date -u -d '90 seconds ago' +%Y-%m-%dT%H:%M:%SZ)
  log "injecting drift line: model=${INJECT_MODEL} ts=${inject_ts}"
  printf '{"type":"assistant","sessionId":"%s","timestamp":"%s","cwd":"%s","message":{"model":"%s"}}\n' \
    "$SESSION_ID" "$inject_ts" "$CWD" "$INJECT_MODEL" >> "$SESSION_FILE"
fi

# --- 5. assert drift is visible in the transcript ---------------------------
log "watching for Fable->non-Fable drift..."
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
  if [ "$CS_DRIFT_SOURCE" = "inject" ]; then
    fail "injected drift line never became the transcript's newest model — probe bug"
  fi
  inconclusive "safety fallback never fired — transcript stayed on ${TARGET_MODEL}. Pick a stronger trigger."
fi
DRIFT_AT=$(date +%s)
log "DRIFT observed: session downgraded ${TARGET_MODEL} -> ${DRIFT_MODEL}"

# --- 6. assert the armed daemon remediated ----------------------------------
log "waiting for daemon remediation (fast-path chain for $SHORT_ID)..."
REMEDIATION_LINE=""
SWITCHED_BACK=""
INTERRUPT_SEEN=""
FOLLOWUP_SENT=""
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
  # In inject mode the drift line stays newest in the file until a real
  # exchange happens, so /model landing is invisible in the transcript.
  # Once the chain has been emitted and had time to land (queued /compact
  # runs first), elicit one fresh assistant message to record the live model.
  if [ "$CS_DRIFT_SOURCE" = "inject" ] && [ -n "$REMEDIATION_LINE" ] \
     && [ -z "$FOLLOWUP_SENT" ] && [ "$(date +%s)" -ge $(( ACT_AT + 20 )) ]; then
    log "eliciting post-remediation assistant line..."
    herdr pane send-text "$PANE" "Reply with exactly one word: done" >/dev/null
    sleep 1
    herdr pane send-keys "$PANE" Enter >/dev/null
    FOLLOWUP_SENT=1
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
log "PASS: ${CS_DRIFT_SOURCE}-mode drift remediated by the armed daemon; back on ${TARGET_MODEL}"
exit 0

#!/usr/bin/env bash
set -euo pipefail

COUNTERSPELL_BIN="${COUNTERSPELL_BIN:-counterspell}"

if ! command -v "$COUNTERSPELL_BIN" >/dev/null 2>&1; then
  echo "● Counterspell | color=#ff453a"
  echo "---"
  echo "counterspell not found in PATH"
  echo "Set COUNTERSPELL_BIN to the installed binary path."
  exit 0
fi

json_file="$(mktemp -t counterspell-status.XXXXXX)"
error_file="$(mktemp -t counterspell-status-error.XXXXXX)"
trap 'rm -f "$json_file" "$error_file"' EXIT

if ! "$COUNTERSPELL_BIN" status --json >"$json_file" 2>"$error_file"; then
  echo "● Counterspell | color=#ff453a"
  echo "---"
  echo "Counterspell stopped or Herdr unreachable | color=#ff453a"
  cat "$error_file"
  echo "---"
  echo "Open Counterspell docs | href=https://github.com/misty-step/counterspell"
  exit 0
fi

python3 - "$json_file" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as handle:
    data = json.load(handle)
summary = data.get("summary", {})
rows = data.get("rows", [])
watched = int(summary.get("watched", 0))
ignored = int(summary.get("ignored", 0))
mapped = int(summary.get("mapped", 0))
live_panes = int(summary.get("live_panes", 0))
last = summary.get("last_trigger_event") or "never"
color = "#32d74b" if watched else "#8e8e93"

print(f"● {watched} watched | color={color}")
print("---")
print(f"Counterspell running | color={color}")
print(f"Watched sessions: {watched}")
print(f"Ignored sessions: {ignored}")
print(f"Mapped transcript sessions: {mapped}")
print(f"Live Claude pane-only rows: {live_panes}")
print(f"Last trigger: {last}")
print("---")

for row in rows[:12]:
    session = row.get("session_id", "-")
    pane = row.get("pane", "-")
    watch = row.get("watch", "-")
    model = row.get("model", "-")
    updated = row.get("updated", "-")
    print(f"{session}  {watch}  {pane}  {model}  {updated}")

if len(rows) > 12:
    print(f"... {len(rows) - 12} more rows")

print("---")
print("Refresh | refresh=true")
print("Open Counterspell | href=https://github.com/misty-step/counterspell")
PY

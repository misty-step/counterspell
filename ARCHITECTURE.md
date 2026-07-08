# Counterspell Architecture

Counterspell is a small Rust CLI around three boundaries:

- Claude transcript JSONL files under `~/.claude/projects/*/*.jsonl`
- live Herdr pane state from `herdr pane list`
- optional extra targets in `~/.counterspell/config.toml`

It has one built-in target model, `claude-fable-5`, and no background daemon.
Every run recomputes state from those boundaries.

## Detection

`counterspell status` and `counterspell watch` discover recent transcript files
from every project directory below `projects_dir` and parse:

- `sessionId`
- `cwd`
- `timestamp`
- model changes from `model` or `message.model`

The transcript `cwd` is path-normalized and matched to Herdr panes by comparing
it with each pane's `cwd` and `foreground_cwd`. Sessions without a matching pane
stay visible as `not-open`. Live Claude panes that have no recent transcript row
are also shown as `herdr-live-pane` rows so the operator can see what
Counterspell cannot yet target from transcript state.

If one transcript cwd maps to multiple live panes, Counterspell breaks the tie
when exactly one of the matching panes is focused (`pane.focused == true`);
that pane is treated as the owner and the resolution is shown as
`focused-tiebreak:<pane-id>`. With zero or more than one focused pane, the row
is still visible but armed remediation is blocked as `ambiguous-pane:<count>`.
Counterspell will not guess which pane owns a session absent that signal.

Detection is allowed to observe every recent session. Observation alone never
authorizes remediation.

## Targeting

Counterspell automatically watches any recent Claude Code session whose
transcript model history includes `claude-fable-5`. The match is history-based,
not latest-model-only, so a session remains watched after it drifts from Fable
to another model.

Config entries under `[[targets]]` are optional extra coverage:

```toml
[[targets]]
cwd_pattern = "/Users/example/work/project"
target_model = "claude-fable-5"
```

Each target has exactly one selector:

- `session_id`
- `project_pattern`
- `cwd_pattern`

and exactly one `target_model`. The automatic Fable target takes precedence over
configured targets. Sessions that have never run Fable and do not match a
configured target are ignored, including deliberate Sonnet or Opus sessions.
This is the safety property the rest of the design protects.

## Gating

Sessions bind to panes by the Herdr-reported `agent_session` id (the Claude
integration's SessionStart hook). A pane bound to the session id is
authoritative; cwd matching is only a fallback, excludes panes bound to a
different session, and any residual multi-pane ambiguity hard-blocks. Focus
never routes keystrokes.

For a drifted session on an **idle** pane, `counterspell watch` checks three
unattended gates:

- transcript quiet: the transcript has not changed inside the quiet window
- pane idle: Herdr reports the mapped pane as `idle`
- debounce: Counterspell has not recently armed remediation for that session

Plain `counterspell watch` is a dry-run and prints the planned action. It does
not write debounce state and does not send text to Herdr.

`counterspell watch --arm` executes only plans that pass all gates.

## Master Switch And Session Overrides

Counterspell exposes two write surfaces for turning enforcement off, and both
are the stable contract any app (CLI, dashboard, or the Tauri desktop app)
writes to. Neither surface shells out to `launchctl` from a request path;
daemon lifecycle is a separate, deliberately terminal-only concern.

**Global master switch — a marker file.** Presence of the marker at
`~/.counterspell/disarmed` (overridable with `--disarm-marker` or
`COUNTERSPELL_DISARM_MARKER`) is the single global off switch. Semantics:

- **Absent marker means ENABLED** — the pre-master-switch default, so existing
  installs and the live daemon keep enforcing until someone opts into
  disabling. The file's contents are a human-readable timestamp only; the gate
  checks presence, never contents.
- While the marker exists, `counterspell watch --arm` is **demoted to a
  detection-only dry-run**: it still detects and logs drift to the bridge feed
  (a paused window is never dark), but never plans a remediation into
  keystrokes. The gate is the single `if arm` guard around `execute_remediation`
  in `watch_rows`; disabling forces `arm` false for the whole pass.

Write it three ways, all flipping the same marker:

| Surface | Command / route | Touches launchd? |
| --- | --- | --- |
| CLI disable | `counterspell disable` | no (marker only) |
| CLI enable | `counterspell enable` | **yes** — also un-disables and (re)loads the watch-arm LaunchAgent so a cleared flag actually results in ticks |
| Dashboard | `POST /master/disable`, `POST /master/enable` | **never** — flag only (`master::enable_flag_only`); a browser request can never reach `launchctl` |

Because the dashboard toggle is flag-only, it is pause/resume for an
already-loaded daemon. Reviving a cold (unloaded or `launchctl disable`d)
daemon is a deliberate terminal action (`counterspell enable`, or
`install-ui`), never a click. The dashboard therefore shows **both axes** —
the flag state and whether the watch-arm daemon is actually
installed+scheduled (`watch_arm_daemon_status`, a read-only `launchctl print`)
— and warns loudly on the one dangerous combination: flag ENABLED but daemon
not scheduled, where nothing will actually run. `counterspell status` and
`doctor` print the flag state and marker path.

**Per-session overrides.** Independent of the global switch, individual
`session_id` targets can be enabled/disabled without editing config: the
dashboard's `POST /targets/enable` and `POST /targets/disable` routes (and the
`counterspell target` subcommand) write the per-session policy. All mutating
dashboard routes require a same-origin `Origin`/`Referer` (CSRF guard); a
missing or mismatched origin returns 403 and changes nothing.

## Fast Path: Act While The Downgraded Turn Is Still Running

Waiting for idle would let a downgraded session finish its whole turn on the
wrong model. When drift shows on a **working** pane that is bound to the exact
session id, the armed watch immediately types the `/compact ...` handoff into
the pane. Claude Code queues composer input submitted mid-turn and executes it
the moment the turn ends, so the compact lands at the earliest possible
boundary. The pass records `pending_compact_unix` and later passes report
`compact-pending` instead of re-queueing.

Once the pane shows idle with a pending compact behind it, the watch sends the
bare `/model <target_model>` switch — skipping the second compact and skipping
the transcript-quiet gate (the recent transcript activity is our own compact).
A bare `/model` on a large uncompacted context pops a cache-rewind
confirmation dialog in Claude Code; switching only after a compact is what
keeps the switch dialog-free. A pending compact expires after 30 minutes, and
the session falls back to the ordinary idle path.

The fast path never fires on `blocked` or `unknown` panes (a blocked pane
usually means a permission prompt is open — injected text would answer it) and
never on cwd-fallback matches.

## Compact Then Switch

The armed action sequence on an idle pane is:

1. Send a plain `/compact ...` command to the mapped Herdr pane.
2. Wait for Herdr to report the pane as `idle`.
3. Send `/model <target_model>` to the same pane.
4. Record `last_action_unix` in `~/.counterspell/sessions.json`.

The debounce clock starts at the model switch; a queued fast-path compact is
tracked by `pending_compact_unix` instead, so the follow-up switch is never
debounced away.

The compaction prompt uses plain framing deliberately. Before moving a session
back to the target model, Counterspell asks Claude to preserve the current goal,
repo/session state, exact next action, and risks in a factual compact handoff.
This reduces context-loss damage from a model switch and avoids asking the model
to infer a hidden policy from clever wording.

## UI And Indicators

`counterspell ui` serves a local Herdr control panel from the Rust CLI itself.
It does not require SwiftBar, xbar, npm, or a separate frontend server. Every
page load recomputes state from automatic Fable targets, configured targets,
transcript JSONLs, and
Herdr workspace/tab/pane lists.

The dashboard is an operator surface, not a remediation path. It mirrors Herdr
as a column drilldown: workspace -> Claude Code tab/pane -> recent transcript
session -> policy. Automatic Fable sessions show as auto-watched. Configured
matches are shown separately, and direct `session_id` overrides can still be
removed from the UI.

`counterspell status --json` emits a summary and row list for external
indicators. The SwiftBar/xbar plugin in `extras/swiftbar/` uses that JSON to
render:

- running/stopped dot
- watched-session count
- last trigger event

For Herdr-native indication, `counterspell --annotate-herdr` recomputes watched
sessions and writes pane metadata with:

```sh
herdr pane report-metadata <pane> --source counterspell --title ... --custom-status ... --ttl-ms 300000
```

This is intentionally TTL-scoped metadata, not a permanent pane rename.

`counterspell install-ui` installs both local indicator surfaces:

- the SwiftBar/xbar plugin under `~/Library/Application Support/SwiftBar/Plugins`
- a LaunchAgent that periodically runs `counterspell --annotate-herdr`

## Current Limits

- Herdr is required for pane discovery and armed injection.
- There is no tmux backend yet.
- Herdr exposes title/custom-status metadata, not a dedicated badge API.
- Same-cwd multi-pane sessions are visible but blocked from armed remediation
  until Counterspell has a precise session-to-pane signal.

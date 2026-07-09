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

For a drifted session on a remediable pane, `counterspell watch` checks the
safe routing gates before it sends text:

- transcript quiet: the transcript has not changed inside the quiet window
- pane safe: Herdr reports a single mapped pane as `idle` or `done`, or a
  `working` pane is bound to the exact session id and can be interrupted
- chain state: no active remediation chain is already in flight

Plain `counterspell watch` is a dry-run and prints the planned action. It does
not write chain state and does not send text to Herdr.

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
  detection-only dry-run**: it still detects and logs drift to the activation
  stream (a paused window is never dark), but never plans a remediation into
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

## Interrupt Chain

Waiting for idle would let a downgraded session finish its whole turn on the
wrong model. When drift shows on a **working** pane that is bound to the exact
session id, the armed watch sends Escape immediately, then sends the plain
`/compact ...` handoff. It does not queue a model switch behind the old turn.

Every remediation is tracked by durable per-session `remediation_chain` state
in `~/.counterspell/sessions.json`: target model, started time, last sent step,
and when that step was sent. The step sequence is:

1. `interrupt_sent`
2. `compact_sent`
3. `switch_sent`
4. `continue_sent`

The state is written before each Herdr send. Repeated armed passes while a
chain is in flight render `remediation-in-flight:<step>` and send nothing
again. A compact only advances to `/model <target_model>` after transcript
evidence shows a compact summary after the chain started. The switch and
`continue` command are then sent together, and the chain is not resolved until
the transcript shows both that compact summary and a post-chain line on the
target model. Only then is `remediation_chain` cleared and `last_action_unix`
updated.

If a step is stuck past the timeout, the next armed pass reports
`remediation-timed-out:<step>` and recovers from recorded state: it resumes the
next unsent step when possible, or restarts the interrupt+compact step with a
timeout reason recorded in the activation stream. It never re-sends compact
while a non-timed-out chain is in flight.

The chain never fires on `blocked` panes (a blocked pane usually means a
permission prompt is open — injected text would answer it). Chain advancement
and timeout recovery require the pane to be bound by reported session id.

The compaction prompt uses plain framing deliberately. Before moving a session
back to the target model, Counterspell asks Claude to preserve the current goal,
repo/session state, exact next action, and risks in a factual compact handoff.
This reduces context-loss damage from a model switch and avoids asking the model
to infer a hidden policy from clever wording.

## Activation Stream

Session-routing telemetry (drift detected, interrupt/compact/model/continue
sent, timeout recovery, remediation confirmed, ignored) is written as JSONL to Counterspell's own
dedicated stream at `~/.counterspell/events.jsonl` (overridable with
`COUNTERSPELL_EVENTS_PATH`), size-rotated to `events.jsonl.1`. It is
deliberately NOT written to the shared fleet feed dir (`~/.factory-lanes/feed`)
— high-frequency internal telemetry there polluted the fleet event feed and
made every consumer pay the parse cost (counterspell-910). The desktop app
tails this stream for its activation log; `api::activation_log` reads it back
and formats each entry in plain, outcome-stamped words.

## Desktop App

`desktop/` is a Tauri v2 app — a persistent, branded control window plus a
native tray icon that supersedes the SwiftBar plugin (counterspell-906). Its
Rust backend consumes the `counterspell` crate as a library through the public
`counterspell::api` surface (`status_snapshot`, `activation_log`, `set_master`,
`set_session_enabled`, `rebind_pane`, health). The webview frontend is plain
HTML/CSS/JS on vendored `aesthetic` tokens.

The app is an **observer + controller only**. It writes exactly the two stable
control surfaces this document defines — the global disarm marker (flag-only,
`api::set_master`) and per-session config targets (`api::set_session_enabled`)
— and it re-asserts a pane's Herdr binding via the same
`pane.report_agent_session` path `rebind` uses (`api::rebind_pane`, the
first-class remote rebind for counterspell-917). It NEVER invokes `launchctl`
or loads/unloads daemons: closing the window (or quitting the app) leaves the
headless `watch --arm` daemon enforcing, which is the whole point. The single
protection verdict — SHIELDED / ACTING / DRIFT-BLOCKED(reason) / DISARMED — is
derived from the live roster and the master-switch state, gated on live panes
so a session that drifted and then closed is history, not a current alarm.

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

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

For an auto-targeted or configured session whose latest transcript model differs
from the target model, `counterspell watch` checks three unattended gates:

- transcript quiet: the transcript has not changed inside the quiet window
- pane idle: Herdr reports the mapped pane as `idle`
- debounce: Counterspell has not recently armed remediation for that session

Plain `counterspell watch` is a dry-run and prints the planned action. It does
not write debounce state and does not send text to Herdr.

`counterspell watch --arm` executes only plans that pass all gates.

## Compact Then Switch

The armed action sequence is:

1. Send a plain `/compact ...` command to the mapped Herdr pane.
2. Wait for Herdr to report the pane as `idle`.
3. Send `/model <target_model>` to the same pane.
4. Record `last_action_unix` in `~/.counterspell/sessions.json`.

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

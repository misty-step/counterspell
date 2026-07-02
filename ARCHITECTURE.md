# Counterspell Architecture

Counterspell is a small Rust CLI around three boundaries:

- Claude transcript JSONL files under `~/.claude/projects/*/*.jsonl`
- live Herdr pane state from `herdr pane list`
- an explicit opt-in TOML config at `~/.counterspell/config.toml`

It has no global target model and no background daemon. Every run recomputes
state from those boundaries.

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

Detection is allowed to observe every recent session. Observation alone never
authorizes remediation.

## Opt-In Targeting

Only config entries under `[[targets]]` are watchable:

```toml
[[targets]]
cwd_pattern = "/Users/example/work/project"
target_model = "claude-fable-5"
```

Each target has exactly one selector:

- `session_id`
- `project_pattern`
- `cwd_pattern`

and exactly one `target_model`. Everything else is ignored, including deliberate
Sonnet or Opus sessions. This is the safety property the rest of the design
protects.

## Gating

For a targeted session whose latest transcript model differs from the target
model, `counterspell watch` checks three unattended gates:

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

## Indicator

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

## Current Limits

- Herdr is required for pane discovery and armed injection.
- There is no tmux backend yet.
- Herdr exposes title/custom-status metadata, not a dedicated badge API.

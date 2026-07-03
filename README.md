# Counterspell

Counterspell watches recent Claude transcript sessions, maps them to live Herdr
panes, and automatically keeps Fable Claude Code sessions on Fable.

## Install

```sh
cargo install --path .
```

## Quickstart

1. Run guided setup:

```sh
counterspell setup --install-ui
```

2. Review discovered sessions and pane mapping:

```sh
counterspell doctor
counterspell status
```

3. Open the local dashboard:

```sh
counterspell ui
```

It serves a browser UI on `127.0.0.1`, opens it by default, and refreshes the
live Herdr Claude Code panes. Fable sessions are marked active automatically;
non-Fable sessions stay inactive unless you add a configured override.

4. Run the armed watch pass:

```sh
counterspell watch --arm
```

Plain `counterspell watch` is a dry-run. It reports eligible compact/switch
actions without sending text to Herdr or writing debounce state.

For an explicit override, add a configured target:

```sh
counterspell target add --session-id db72af91-c78f-4b3f-80be-6dca7c264f75
counterspell target list
```

`target add` defaults to `claude-fable-5`; pass `--target-model` only when an
override should enforce a different model.

## Config

Default config path: `~/.counterspell/config.toml`.

Counterspell has one built-in automatic policy: any recent Claude Code
transcript whose model history includes `claude-fable-5` is watched with
`claude-fable-5` as the desired model. That keeps a session active after it
drifts away from Fable, which is when Counterspell needs to act.

Configured `[[targets]]` entries are still supported for overrides and extra
coverage. Each entry must set exactly one selector and one explicit
`target_model`:

```toml
[[targets]]
session_id = "db72af91-c78f-4b3f-80be-6dca7c264f75"
target_model = "claude-fable-5"

[[targets]]
project_pattern = "-Users-phaedrus-Development-adminifi*"
target_model = "claude-fable-5"

[[targets]]
cwd_pattern = "/Users/phaedrus/Development/adminifi/*"
target_model = "claude-fable-5"
```

Selectors are `session_id`, `project_pattern`, or `cwd_pattern`. Patterns
support `*`. Sessions that have never run Fable and do not match a configured
target are ignored.

## UI And Indicators

For a visible local UI, run:

```sh
counterspell ui
```

The dashboard is a Herdr Mirror column drilldown: choose a workspace, choose a
Claude Code tab/pane, inspect recent transcript sessions for that pane cwd, and
see whether each session is auto-watched by the Fable policy. `counterspell ui
--no-open` starts the same server without launching a browser.

Automatic Fable sessions show as `Auto`. Explicit config matches show as
configured. Sessions that have never run Fable show as inactive.

Counterspell ships a SwiftBar/xbar plugin that reads `counterspell status
--json` and renders a menu-bar dot, watched-session count, and last trigger
event.

Install the menu-bar plugin and a LaunchAgent that periodically annotates Herdr
panes:

```sh
counterspell install-ui --load
```

Or install only the SwiftBar plugin manually:

```sh
mkdir -p "$HOME/Library/Application Support/SwiftBar/Plugins"
cp extras/swiftbar/counterspell.5m.sh "$HOME/Library/Application Support/SwiftBar/Plugins/"
chmod +x "$HOME/Library/Application Support/SwiftBar/Plugins/counterspell.5m.sh"
```

If `counterspell` is not on SwiftBar's PATH, set `COUNTERSPELL_BIN` in the
script or in SwiftBar's environment.

For Herdr-native indication, run:

```sh
counterspell --annotate-herdr
```

Counterspell uses `herdr pane report-metadata` with source `counterspell` to set
a short-lived title/custom status on watched panes. This does not permanently
rename panes.

If more than one live Herdr pane maps to the same transcript cwd, Counterspell
shows `ambiguous-pane:<count>` and will not arm remediation for that session.
Use session-specific targets for visibility, and avoid `watch --arm` until the
pane mapping is unique.

## Scope

`counterspell status` discovers recent `~/.claude/projects/*/*.jsonl`
transcripts, runs `herdr pane list`, maps sessions to panes by cwd, and shows
mapped sessions, unmapped sessions, and live Claude panes without a recent
transcript.

The armed remediation path is scoped to Herdr terminal panes. `watch --arm`
sends a plain `/compact ...` handoff, waits for the pane to become idle, then
sends `/model <target_model>`. No tmux backend is included yet; that is a filed
follow-up. Deliberate Sonnet/Opus sessions remain untouched unless they have
previously run Fable or are explicitly targeted in config.

See [ARCHITECTURE.md](ARCHITECTURE.md) for detection vs arming, gating, and the
compact-then-switch sequence.

## Verification

```sh
cargo fmt -- --check && cargo test && cargo clippy --all-targets -- -D warnings
cargo install --path .
counterspell doctor
counterspell status
counterspell status --json
counterspell watch --arm
```

## License

MIT. See [LICENSE](LICENSE).

Copyright (c) 2026 Misty Step LLC.

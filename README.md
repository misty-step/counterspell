# Counterspell

Counterspell watches recent Claude transcript sessions, maps them to live Herdr
panes, and only arms model-correction actions for explicitly configured targets.

## Install

```sh
cargo install --path .
```

## Quickstart

Run these from the project directory you want Counterspell allowed to touch.
The first command creates an explicit opt-in target; no other sessions can be
armed.

1. Run guided setup:

```sh
counterspell setup --cwd-pattern "$PWD" --install-ui
```

2. Review discovered sessions and pane mapping:

```sh
counterspell doctor
counterspell status
```

3. Run the armed watch pass:

```sh
counterspell watch --arm
```

Plain `counterspell watch` is a dry-run. It reports eligible compact/switch
actions without sending text to Herdr or writing debounce state.

For an exact live conversation, prefer a session target:

```sh
counterspell target add --session-id db72af91-c78f-4b3f-80be-6dca7c264f75
counterspell target list
```

`target add` defaults to `claude-fable-5`; pass `--target-model` when a target
should enforce a different model.

## Config

Default config path: `~/.counterspell/config.toml`.

Counterspell is strictly opt-in. There is no global target model. Unmatched
sessions are always ignored, including deliberate Sonnet or Opus sessions.

Each `[[targets]]` entry must set exactly one selector and one explicit
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
support `*`. There is no global target model: sessions that do not match a
target are ignored, even when Counterspell observes model drift.

## Indicator

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
follow-up. Deliberate Sonnet/Opus sessions remain untouched unless they are
explicitly targeted in config.

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

# Counterspell

Counterspell watches recent Claude transcript sessions, maps them to live Herdr
panes, and only arms model-correction actions for explicitly configured targets.

## Install

```sh
cargo install --path .
```

## Quickstart

1. Initialize the opt-in config:

```sh
mkdir -p ~/.counterspell && cat > ~/.counterspell/config.toml <<'TOML'
recent_hours = 72
transcript_quiet_seconds = 30
debounce_seconds = 300

[[targets]]
project_pattern = "-Users-phaedrus-Development-adminifi*"
target_model = "claude-fable-5"
TOML
```

2. Review discovered sessions and pane mapping:

```sh
counterspell status
```

3. Run the armed watch pass:

```sh
counterspell watch --arm
```

Plain `counterspell watch` is a dry-run. It reports eligible compact/switch
actions without writing debounce state.

## Config

Default config path: `~/.counterspell/config.toml`.

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

## Scope

`counterspell status` discovers recent `~/.claude/projects/*/*.jsonl`
transcripts, runs `herdr pane list`, maps sessions to panes by cwd, and shows
mapped sessions, unmapped sessions, and live Claude panes without a recent
transcript.

The armed remediation path is scoped to Herdr terminal panes. This release
reports the compact/switch sequence and records debounce state for eligible
targets; Counterspell uses Herdr for pane discovery and any injection workflow.
No tmux backend is included yet; that is a filed follow-up. Deliberate
Sonnet/Opus sessions remain untouched unless they are explicitly targeted in
config.

## Verification

```sh
cargo fmt -- --check && cargo test && cargo clippy --all-targets -- -D warnings
cargo install --path .
counterspell status
counterspell watch --arm
```

## License

MIT. See [LICENSE](LICENSE).

Copyright (c) 2026 Misty Step LLC.

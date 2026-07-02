# Counterspell

Counterspell observes recent Claude transcript sessions and maps them to Herdr
panes.

## Commands

```sh
counterspell status
counterspell watch
```

`counterspell status` discovers recent `~/.claude/projects/*/*.jsonl`
transcripts, runs `herdr pane list`, maps sessions to panes by matching cwd, and
shows every recent session as `watched` or `ignored`. Unmapped sessions remain
visible as `not-open`; live Claude panes without a recent transcript are shown
as `herdr-live-pane`.

`counterspell watch` runs one detection/gating pass. It may plan remediation
only for explicitly configured targets. The global default is unwatched, so
deliberate Sonnet/Opus sessions are observed but never forced to another model
unless a config entry targets that session or project.

## Config

Default config path: `~/.counterspell/config.toml`.

```toml
recent_hours = 72
transcript_quiet_seconds = 30
debounce_seconds = 300

[[targets]]
project_pattern = "-Users-phaedrus-Development-adminifi*"
target_model = "claude-fable-5"

[[targets]]
session_id = "db72af91-c78f-4b3f-80be-6dca7c264f75"
target_model = "claude-fable-5"

[[targets]]
cwd_pattern = "/Users/phaedrus/Development/adminifi/*"
target_model = "claude-fable-5"
```

Each target must set exactly one selector: `session_id`, `project_pattern`, or
`cwd_pattern`. Patterns support `*`. There is no global target model.

## Verification

```sh
cargo fmt -- --check && cargo test && cargo clippy --all-targets -- -D warnings
cargo install --path .
counterspell status
```

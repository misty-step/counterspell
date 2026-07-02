# Counterspell

Counterspell watches Codex sessions and shows where they live in Herdr.

## Commands

```sh
counterspell watch
counterspell status
```

`counterspell watch` records the current session id and current working
directory. It prefers `COUNTERSPELL_SESSION_ID`, then `CODEX_THREAD_ID`, then
`CODEX_SESSION_ID`, and falls back to `agent_session.value` from a matching
`herdr pane list` pane. If no current Codex session can be found, it exits with
an error instead of inventing one.

`counterspell status` reads the watch list, runs `herdr pane list`, and maps
each watched session to panes whose `cwd` or `foreground_cwd` equals the watched
cwd after path normalization. Watched sessions with no live matching pane are
shown as `not-open`; Herdr command or JSON failures are errors.

The default state file is `~/.counterspell/sessions.json`. Use `--state PATH`
or `COUNTERSPELL_STATE=PATH` for tests or isolated runs.

## Verification

The repo gate is:

```sh
cargo fmt -- --check && cargo test && cargo clippy --all-targets -- -D warnings
```

The live operator check is:

```sh
cargo install --path .
counterspell watch
counterspell status
```

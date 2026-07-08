# Counterspell Repo Contracts

- Gate: `cargo fmt -- --check && cargo test && cargo clippy --all-targets -- -D warnings`.
- This repo ships a Rust CLI. Keep non-Rust code out unless it is a tiny external process fixture or shell boundary.
- `counterspell status` must discover recent Claude transcript JSONLs from configured `projects_dir`, resolve sessions to Herdr panes by running `herdr pane list`, and show watched vs ignored status.
- Counterspell auto-watches live Claude Code sessions whose transcript history includes the default Fable model. This auto-Fable target takes precedence over config. `watch --arm` may remediate those auto targets or additional explicit `[[targets]]` entries. Drift on a session-bound working pane must interrupt immediately, send the plain-handoff compact, advance to `/model claude-fable-5` only after compact-summary evidence, then send `continue`; durable `remediation_chain` state prevents duplicate sends across repeated armed passes. Pane binding is by reported agent session id — focus and cwd guesses must never route keystrokes.
- Runtime remediation/debounce state must live outside the repo by default. Tests may override it with `--state` or `COUNTERSPELL_STATE`.

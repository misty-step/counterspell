# Counterspell Repo Contracts

- Gate: `cargo fmt -- --check && cargo test && cargo clippy --all-targets -- -D warnings`.
- This repo ships a Rust CLI. Keep non-Rust code out unless it is a tiny external process fixture or shell boundary.
- `counterspell status` must discover recent Claude transcript JSONLs from configured `projects_dir`, resolve sessions to Herdr panes by running `herdr pane list`, and show watched vs ignored status.
- Counterspell is strictly opt-in for remediation: default is unwatched, and `watch` may only arm actions for explicit `[[targets]]` config entries with a `target_model`.
- Runtime debounce state must live outside the repo by default. Tests may override it with `--state` or `COUNTERSPELL_STATE`.

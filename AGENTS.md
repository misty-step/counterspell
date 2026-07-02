# Counterspell Repo Contracts

- Gate: `cargo fmt -- --check && cargo test && cargo clippy --all-targets -- -D warnings`.
- This repo ships a Rust CLI. Keep non-Rust code out unless it is a tiny external process fixture or shell boundary.
- `counterspell status` must resolve watched sessions to Herdr panes by running `herdr pane list` and matching each watched cwd to pane cwd values.
- Runtime watch state must live outside the repo by default. Tests may override it with `--state` or `COUNTERSPELL_STATE`.


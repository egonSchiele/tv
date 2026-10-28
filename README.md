# tv

[![CI](https://github.com/egonSchiele/tv/actions/workflows/ci.yml/badge.svg)](https://github.com/egonSchiele/tv/actions/workflows/ci.yml)

`tv` is a terminal table viewer that a human and an agent share. The agent pushes a table and the human sees it in a proper viewer with scrolling, colors, and truncation. The agent can see which cell the human is looking at, the human can comment on a cell, and the agent reads and replies to comments. Either side can save the table (with comments) as JSON and reopen it later. See [SPEC.md](SPEC.md) for the full design.

## Development

Install Rust with [rustup](https://rustup.rs). `rust-toolchain.toml` pins the toolchain, so rustup picks the right version automatically.

| Command | What it does |
|---|---|
| `make` | Build (`cargo build`). |
| `make run` | Run the viewer on `examples/prs.json`. |
| `make test` | Run tests (`cargo test`). |
| `make fmt` | Format the code with rustfmt (Prettier equivalent). |
| `make lint` | Lint with clippy (ESLint equivalent), treating warnings as errors. `cargo clippy --fix` auto-fixes many lints. |
| `make check` | Run what CI runs: format check, lint, tests. |

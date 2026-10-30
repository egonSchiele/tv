# tv

[![CI](https://github.com/egonSchiele/tv/actions/workflows/ci.yml/badge.svg)](https://github.com/egonSchiele/tv/actions/workflows/ci.yml)

`tv` is a terminal table viewer that a human and an agent share. The agent pushes a table and the human sees it in a proper viewer with scrolling, colors, and truncation. The agent can see which cell the human is looking at, the human can comment on a cell, and the agent reads and replies to comments. Either side can save the table (with comments) as JSON and reopen it later. See [docs/SPEC.md](docs/SPEC.md) for the full design.

## Install

From a clone of this repo, with [Rust](https://rustup.rs) installed:

```sh
make install
```

This builds an optimized `tv` and copies it to `~/.bin`, which needs to be on your `PATH`. To install somewhere else, run `make install BINDIR=<dir>`. Run it again to update.

## Quick start

```sh
gh pr list --json number,title,author | tv -   # any JSON array of objects
tv data.json                                   # or a file; see docs/schema.md
tv ls                                          # sessions, most recent first
tv                                             # reopen the most recent session
```

Press `?` in the viewer for every key and command. In a clone of this repo, `make run` opens `examples/prs.json`. [docs/schema.md](docs/schema.md) describes the table format.

For agents: `tv agent help` prints a guide to using tv, and `tv skills install tv-tables --to .claude/skills` gives your agent a skill that teaches it.

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
| `make install` | Build an optimized `tv` and copy it to `~/.bin` (or `BINDIR`). |

## Dependencies

Every crate we depend on, and everything it pulls in, is code we download and often run at build time (build scripts and proc macros run during `cargo build`, and also when your editor's rust-analyzer opens the project). crates.io has had malicious releases, so we keep the list short:

- Prefer the standard library or a few lines of our own code over a new crate.
- When a crate is worth it, turn off default features and enable only what we use.
- Check what a new crate adds: `cargo tree -e normal,build,dev --prefix none | sort -u | wc -l`.
- `Cargo.lock` is committed and CI builds with `--locked`, so nothing changes versions without a commit.
- `.cargo/config.toml` makes cargo skip crate versions less than 7 days old when it resolves new versions (needs Rust 1.100+).

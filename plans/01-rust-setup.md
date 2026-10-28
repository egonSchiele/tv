# Plan: ADI-197 Initial Rust setup

Linear: https://linear.app/aditya-bhargava/issue/ADI-197
Sub-issues: ADI-205 (toolchain + init), ADI-206 (Makefile + example), ADI-207 (fmt + clippy), ADI-208 (CI).

## Goal

A Rust package in `~/tv` where `make`, `make run`, and `make test` work, formatting and linting are configured, and GitHub Actions fails on any fmt, clippy, or test error. The owner is new to Rust, so leave short explanatory comments where a tool is introduced.

## Context for the executing agent

- Read `SPEC.md` first, especially "Implementation".
- The repo is not yet a git repository and has no Rust toolchain installed. `~/.cargo/bin` does not exist.
- Nothing here depends on later tickets. Produce a compiling `main` that prints a placeholder; later tickets replace it.
- Do not create a GitHub repo or push. Leave CI as a file in the tree; the owner will push.

## Steps

### 1. Install the toolchain (ADI-205)

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile default
source "$HOME/.cargo/env"
rustc --version && cargo --version
```

`default` profile includes `rustfmt` and `clippy`. If the install cannot run non-interactively in your environment, stop and ask the owner to run it with `! curl ... | sh`.

### 2. Initialize the package (ADI-205)

```sh
cd ~/tv
git init
cargo init --name tv
```

Then edit `Cargo.toml` to:

```toml
[package]
name = "tv"
version = "0.1.0"
edition = "2024"
description = "A terminal table viewer you share with an agent"
license = "MIT"

[[bin]]
name = "tv"
path = "src/main.rs"

[dependencies]
# added via cargo add below

[lints.clippy]
all = "warn"
pedantic = "warn"
# Pedantic lints that fight normal code; re-enable individually if wanted.
module_name_repetitions = "allow"
must_use_candidate = "allow"
missing_errors_doc = "allow"
missing_panics_doc = "allow"
```

If `edition = "2024"` is rejected by the installed compiler, use `"2021"`.

Add dependencies with `cargo add` so versions resolve to current releases:

```sh
cargo add ratatui crossterm anyhow serde_json unicode-width dirs rand chrono
cargo add serde --features derive
cargo add clap --features derive
cargo add --dev insta --features json
cargo add --dev assert_cmd tempfile predicates
```

Check that the `ratatui` version pulled in is compatible with the `crossterm` version (ratatui re-exports crossterm; prefer `ratatui::crossterm` in code and consider dropping the direct crossterm dep if ratatui's re-export covers the needed types). Record the resolved versions in the PR description so later tickets know which API they are on.

Add `rust-toolchain.toml`:

```toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy"]
```

`.gitignore`:

```
/target
```

Commit `Cargo.lock`. It is a binary crate, so the lockfile belongs in version control.

### 3. Placeholder main

`src/main.rs` with a clap skeleton that later tickets extend. Keep it minimal but real, so clippy and tests have something to check:

```rust
use clap::{Parser, Subcommand};

/// A terminal table viewer you share with an agent.
#[derive(Parser)]
#[command(name = "tv", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Open a table from a JSON file (sessions come later).
    Open { path: String },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Open { path } => {
            println!("would open {path}");
            Ok(())
        }
    }
}
```

Add one trivial unit test in `main.rs` (e.g. `Cli::command().debug_assert()` from clap, which validates the CLI definition) so `make test` runs at least one test.

### 4. Example data (ADI-206)

Create `examples/prs.json` in the spec's table format: title "Open pull requests", key `number`, 9 columns (`number`, `title`, `author`, `state`, `additions`, `deletions`, `labels`, `updated_at`, `url`), about 40 rows. Make some titles longer than 40 chars so truncation shows, include a `labels` column with array values so nested JSON rendering has something to render, and include a null or two. The data can be invented but should look plausible.

Note: cargo treats `examples/*.rs` as example binaries. A `.json` file there is fine and is ignored by cargo. If that ever causes confusion, move to `fixtures/`.

### 5. Makefile (ADI-206)

```make
.PHONY: all build run test lint fmt fmt-check check clean

all: build

build:
	cargo build

run: build
	cargo run -- open examples/prs.json

test:
	cargo test

lint:
	cargo clippy --all-targets -- -D warnings

fmt:
	cargo fmt

fmt-check:
	cargo fmt --check

# What CI runs.
check: fmt-check lint test

clean:
	cargo clean
```

Use real tabs for recipe indentation.

### 6. rustfmt and clippy config (ADI-207)

`rustfmt.toml`:

```toml
max_width = 100
```

Clippy config lives in `Cargo.toml` `[lints.clippy]` (step 2). Run `cargo clippy --all-targets -- -D warnings` and fix everything it reports in the placeholder. Add a short section to `README.md` under "Development":

- `make fmt` formats (Prettier equivalent).
- `make lint` lints (ESLint equivalent). `cargo clippy --fix` auto-fixes many lints.
- `make check` runs what CI runs.

### 7. GitHub Actions (ADI-208)

`.github/workflows/ci.yml`:

```yaml
name: CI
on:
  push:
    branches: [main]
  pull_request:

env:
  CARGO_TERM_COLOR: always

jobs:
  check:
    strategy:
      matrix:
        os: [ubuntu-latest, macos-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --check
      - run: cargo clippy --all-targets -- -D warnings
      - run: cargo test
```

Each `run` step fails the job on non-zero exit, so no extra configuration is needed for "fail on any error".

### 8. README skeleton

`README.md` with: one-paragraph description from SPEC.md, "Development" section (make targets), and a CI badge placeholder with the repo URL left as `<owner>/tv` for the owner to fill in.

## Acceptance

Run these and paste the output in the PR or final report:

```sh
make check          # fmt-check, clippy with -D warnings, tests: all pass
make run            # prints "would open examples/prs.json"
git status --short  # Cargo.lock tracked, target/ ignored
```

## Deliverables

- `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `rustfmt.toml`, `.gitignore`
- `src/main.rs`
- `examples/prs.json`
- `Makefile`
- `.github/workflows/ci.yml`
- `README.md`
- One commit on `main` (or a branch, if the owner prefers), message: `Initial Rust setup: cargo, make, fmt, clippy, CI`

## Out of scope

Any viewer code. `src/table.rs`, `src/viewer/` etc. come in ADI-198.

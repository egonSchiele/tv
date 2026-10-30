.PHONY: all build run test snapshots lint fmt fmt-check check install clean

all: build

build:
	cargo build

# A session per run, kept out of your real ~/.tv. `make clean` removes them.
run:
	TV_HOME=$(CURDIR)/target/tv-home cargo run -- open examples/prs.json

test:
	cargo test

# Rewrite the render snapshot files. Review the changes with `git diff`.
snapshots:
	UPDATE_SNAPSHOTS=1 cargo test

# Clippy is the linter (like ESLint). -D warnings turns every warning into an error.
lint:
	cargo clippy --all-targets -- -D warnings

# rustfmt is the formatter (like Prettier).
fmt:
	cargo fmt

fmt-check:
	cargo fmt --check

# What CI runs.
check: fmt-check lint test

# Build an optimized tv and copy it to ~/.local/bin. Override with `make install BINDIR=...`.
# `install`, not `cp`: it replaces the old binary instead of writing over it, and
# on Apple silicon a binary overwritten in place can be killed at launch.
BINDIR ?= $(HOME)/.local/bin

install:
	cargo build --release --locked
	install -d "$(BINDIR)"
	install -m 755 target/release/tv "$(BINDIR)/tv"

clean:
	cargo clean

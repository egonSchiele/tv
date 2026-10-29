.PHONY: all build run test snapshots lint fmt fmt-check check clean

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

clean:
	cargo clean

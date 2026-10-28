.PHONY: all build run test lint fmt fmt-check check clean

all: build

build:
	cargo build

run:
	cargo run -- open examples/prs.json

test:
	cargo test

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

.PHONY: all fmt fmt-check test clippy build install check clean

# Run full pipeline: format, lint, test, build, install
all: fmt clippy test build install

# Format code
fmt:
	cargo fmt --all

# Verify formatting without modifying
fmt-check:
	cargo fmt --all -- --check

# Run tests
test:
	cargo test --all-targets

# Lint with clippy (warnings as errors, matches CI)
clippy:
	cargo clippy --all-targets -- -D warnings

# Build release binary
build:
	cargo build --release

# Where the shell finds `ai-tester` first; override with `make install BINDIR=...`
BINDIR ?= /usr/local/bin

# Install binary globally as ai-tester.
# 1. cargo install -> ~/.cargo/bin (--offline reuses the local registry cache,
#    avoids crates.io network/SSL hiccups)
# 2. copy into BINDIR so a /usr/local/bin entry earlier in PATH stays current
install:
	cargo install --path . --force --offline
	cp target/release/ai-tester "$(BINDIR)/ai-tester"
	@echo "installed -> $$(command -v ai-tester)"

# CI-style check: no mutation, fail on issues
check: fmt-check clippy test

# Remove build artifacts
clean:
	cargo clean

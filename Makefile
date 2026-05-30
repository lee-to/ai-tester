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

# Install binary globally as ai-tester (~/.cargo/bin)
# --offline: reuse the local registry cache; avoids crates.io network/SSL hiccups
install:
	cargo install --path . --force --offline

# CI-style check: no mutation, fail on issues
check: fmt-check clippy test

# Remove build artifacts
clean:
	cargo clean

# Contributing to ai-tester

Thanks for your interest in contributing. This project is now a native Rust CLI.

## Requirements

- Rust 1.82 or newer
- `git` on `PATH`
- Optional `claude` and/or `codex` CLIs for manual live runtime checks

## Setup

```bash
git clone https://github.com/lee-to/ai-tester.git
cd ai-tester
cargo build
```

## Tests

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Unit and integration tests must not call real model providers. Use fake CLI executables or golden JSONL fixtures for runtime adapter coverage.

## Project Layout

```text
src/
  cli.rs              # clap command definitions
  commands/           # subcommand implementations
  config.rs           # .ai-tester.yaml discovery
  scenario.rs         # YAML scenario schema and loader
  sandbox.rs          # temp git sandbox and fixtures
  runtime/            # Claude/Codex process adapters and JSONL normalization
  skill/              # SKILL.md parsing and skill discovery helpers
  assertions/         # assertion evaluators
  trace/              # v2 trace records and writer
tests/                # Rust integration tests
```

## Adding A Runtime

Add the process invocation and JSONL normalization in `src/runtime/mod.rs`, then cover it with golden parser tests. Live-provider tests should stay manual unless they can run against a fake executable in CI.

## Adding An Assertion

1. Add a variant to `AssertionSpec` in `src/scenario.rs`.
2. Add the evaluator in `src/assertions/mod.rs`.
3. Add tests that cover pass and fail behavior.
4. Document the assertion in `README.md`.

## Pull Request Checklist

- [ ] `cargo fmt --check` passes.
- [ ] `cargo clippy --all-targets -- -D warnings` passes.
- [ ] `cargo test` passes.
- [ ] User-visible behavior is documented in `README.md`.
- [ ] `CHANGELOG.md` has an entry under `## [Unreleased]`.

## Release Process

Maintainers only:

1. Move changelog entries from `[Unreleased]` into a version heading.
2. Bump `version` in `Cargo.toml`.
3. Tag the commit and push the tag.
4. Let the release workflow build platform artifacts.
5. Publish to crates.io with `cargo publish`.

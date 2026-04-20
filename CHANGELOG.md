# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-04-18

Initial public release.

### Added

- CLI `ai-tester` with subcommands `run`, `runtimes`, `sandbox-prune`, and stubs for `trend` / `compare` / `trace`.
- Project config discovery via `.ai-tester.yaml` — walks up from the current working directory to locate the skills root.
- Three prompt-source modes per scenario: `skill:`, `system_prompt:` (inline), and `system_prompt_file:` (external file).
- Declarative YAML scenarios with fixtures, user responses, and assertions.
- Fixture loaders:
  - Inline `content:` strings.
  - `content_from: <rel-path>` — load a single file's content from disk.
  - `copy_trees: [{from, to?}]` — copy entire directory trees into the sandbox.
- Assertion types: `tool_called`, `tool_call_sequence`, `no_tool_called`, `output_contains`, `turn_count_at_most`, `no_path_escape`. Implicit `no_unanswered_questions` and `turn_budget` always on.
- PCRE-style inline regex flags (`(?i)`, `(?m)`, `(?s)`) supported in patterns.
- Multi-runtime support via a pluggable `RuntimeAdapter` interface:
  - `claude` (default) via `@anthropic-ai/claude-agent-sdk`.
  - `codex` via `@openai/codex-sdk`.
- Isolated git-worktree sandbox per scenario under `$TMPDIR/ai-tester-<scenario>-<rand>/`, with SIGINT/SIGTERM/SIGHUP cleanup and orphan-pruning via `ai-tester sandbox-prune`.
- JSON trace output per run under `runs/<skill-or-inline>/<iso>__<semver>__<hash8>.json` including turns, tool calls, assertions, scoring, and cost estimates.
- Live progress reporter with idle-warning (`--idle-warn`) and `--quiet` mode.
- Weighted scoring (`scoring.weightedScore`) for future trend analysis.

[Unreleased]: https://github.com/lee-to/ai-tester/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/lee-to/ai-tester/releases/tag/v0.1.0

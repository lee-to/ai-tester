# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Assertion `capture` field.** `tool_called` and `tool_call_sequence` specs now accept `capture: [<field>, …]` (plus optional `capture_max_chars: <n>`, default 2000). On pass, the named input fields of the matched tool call are echoed under the assertion line in `=== Results ===` as a dim pipe-quoted block with a `truncated, showing X/Y chars` annotation when capped. Full untruncated values are persisted to `assertions[].captures` in the JSON trace. Handy for inspecting what the agent actually wrote (e.g. the body of `Write(.ai-factory/PLAN.md)`) without opening the raw trace.
- **`runner.setting_sources` (Claude-only).** Opt-in per scenario for loading user/project/local Claude settings — hooks, MCP servers, user-level skills. Values: `user`, `project`, `local` (maps 1:1 to the SDK's `settingSources`). Default (omitted) keeps runs hermetic — no user config leaks into the sandbox. Use when you specifically need parity with the interactive `claude` CLI (e.g. regression-testing a `PreToolUse` hook or a project-local MCP server).

## [0.3.0] - 2026-04-22

### Added

- **Token accounting in results.** Final `=== Results ===` block now reports the aggregated token consumption across all scenarios (input / output / cache-creation / cache-read with a cache-read percentage), alongside the existing USD estimate — meant to make optimization-driven iteration on skills practical.
- **Per-skill & per-scenario token budgets.** New `token-budget: <N>` field in `SKILL.md` frontmatter and `token_budget: <N>` (or `token-budget:` alias) in scenario YAML. Scenario-level budget overrides skill-level when both are set. An implicit `token_budget` assertion fails the run when `input + output + cache-creation + cache-read` exceeds the effective budget; the trace stores both `skill.tokenBudget` and `scenario.tokenBudget` so the effective source is always visible.
- **`ai-tester history` command.** Browse past runs from `runs/` without opening raw JSON. Sorts newest-first and prints one line per run with timestamp, pass/fail, duration, turns, total tokens (annotated with the effective budget when present, plus a red `over-budget` tag when it tripped), and USD estimate. Flags: positional `[skill]`, `--scenario <id>`, `--last <n>`, `--json`.
- **Scripted user turns.** New scenario-level overrides for the opening message, replacing the generic auto-template ("Run the <skill> skill…"):
  - `user_prompt: <string>` — single verbatim kickoff. Useful for driving the agent the way a human would (`/aif-plan <args>` in Claude Code, `$preset` in Codex, or any custom phrasing). When set, `argument` is ignored.
  - `user_prompts: [<string>, …]` — scripted chain of turns delivered sequentially within the same agent session (Claude via `query({ resume: sessionId })`; Codex reuses the same `thread` across `runStreamed()` calls). `session_id` is pinned on the first init and reused for every step. Context, tool history, and side effects accumulate across the chain; budgets and assertions apply to the aggregated run.
  - Mutually exclusive with each other; mixing them is a validation error.
- **Live-progress `scripted_prompt` event.** During chains the reporter prints `▸ [step N/M] "…"` in magenta before each scripted turn so you can tell which prompt the agent is currently working on.

### Changed

- `RuntimeRunRequest.firstUserMessage: string` replaced with `userMessages: string[]` — internal API change for runtime adapters; external scenario YAML is not affected.
- `TraceRecord.skill` gained `tokenBudget`; `TraceRecord.scenario` gained `tokenBudget`. Both default to `null` when unset.
- Claude runtime now runs scripted chains as sequential `query()` calls with `resume: sessionId` instead of a single iterator-based session (iterator multi-turn only works for `tool_use → tool_result` continuations, not `end_turn → next user prompt`).

## [0.2.0] - 2026-04-20

### Added

- `ai-tester init` command that writes a default `.ai-tester.yaml` into the current directory. Supports `--force`, `--skills-dir`, `--model`, and `--permission-mode` overrides.

### Changed

- Renamed the npm package from `@lee-to/ai-tester` to `@cutcode/ai-tester`.
- README quick-start now uses `ai-tester init` instead of a heredoc for writing the config.

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

[Unreleased]: https://github.com/lee-to/ai-tester/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/lee-to/ai-tester/releases/tag/v0.3.0
[0.2.0]: https://github.com/lee-to/ai-tester/releases/tag/v0.2.0
[0.1.0]: https://github.com/lee-to/ai-tester/releases/tag/v0.1.0

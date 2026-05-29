# ai-tester

> End-to-end behavioral testing for **skills**, **bare system prompts**, and agent runtimes. Run real scenarios in an isolated git sandbox, capture normalized tool-call traces, and assert behavior with declarative YAML.

[![license](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)
[![rust](https://img.shields.io/badge/rust-1.82%2B-orange.svg)](https://www.rust-lang.org)
[![CI](https://github.com/lee-to/ai-tester/actions/workflows/ci.yml/badge.svg)](https://github.com/lee-to/ai-tester/actions/workflows/ci.yml)

---

## Why ai-tester?

LLM tests that mock the model are easy to write and weak at catching production failures. The real bugs show up in tool-use sequences, sandbox behavior, permission-mode differences, and the instructions the model actually sees.

`ai-tester` creates a throwaway sandbox per scenario, runs the selected runtime, records every normalized turn and tool call, and evaluates the run against YAML assertions.

## Features

- **Native Rust CLI.** No Node runtime or embedded SDK dependency.
- **Real runs, real tools.** Scenarios execute inside an isolated temporary sandbox.
- **Multi-runtime.** Built-in adapters for Claude Code and OpenAI Codex through their installed CLIs.
- **Three prompt sources.** Test a packaged skill, an inline `system_prompt`, or an external prompt file.
- **Scripted user turns.** Use `user_prompt` or `user_prompts` for custom session flow.
- **Declarative assertions.** `tool_called`, `tool_call_sequence`, `no_tool_called`, `output_contains`, `no_output_contains`, `file_read`, `turn_count_at_most`, and `no_path_escape`.
- **Fixtures.** Inline files, `content_from`, directory trees, staged changes, committed baselines, and setup commands.
- **Trace output.** Every live run writes a schema `2.0.0` JSON trace under `runs/`.
- **History view.** `ai-tester history` summarizes prior v2 traces.
- **Security checks.** Assertions can forbid web tools, shell networking, path escapes, destructive shell commands, or unexpected tool families.

## Installation

```bash
cargo install ai-tester
```

Build from source:

```bash
git clone https://github.com/lee-to/ai-tester.git
cd ai-tester
cargo build --release
./target/release/ai-tester --help
```

Requires Rust **1.82 or newer** when building from source.

## Prerequisites

Per runtime you plan to use:

- **Claude** (`runtime: claude`, default): `claude` CLI installed and logged in.
- **Codex** (`runtime: codex`): `codex` CLI installed and logged in.

Check local readiness:

```bash
ai-tester runtimes
#   claude     ready      Claude Code via `claude -p --output-format stream-json`.
#   codex      ready      OpenAI Codex via `codex exec --json`.
```

## Quick Start

```bash
# 1. Create project config
ai-tester init

# 2. Validate scenarios without calling a runtime
ai-tester run --dry-run

# 3. Run scenarios discovered under skills_dir
ai-tester run

# 4. Run one standalone scenario file
ai-tester run --file ./scenario.yaml --runtime codex

# 5. Run standalone scenarios from a directory
ai-tester run --dir ./prompts --runtime codex
```

## Project Config

`ai-tester` walks upward from the current working directory looking for `.ai-tester.yaml`. If none is found, it falls back to `./skills` in the current directory.

```yaml
skills_dir: ./skills
defaults:
  model: claude-sonnet-4-6
  permission_mode: bypassPermissions
```

With this file at a project root, skills live at:

```text
skills/<skill-name>/SKILL.md
skills/<skill-name>/tests/*.yaml
```

## CLI

```bash
# Create a config
ai-tester init
ai-tester init --force --skills-dir ./agent-skills --model gpt-5-codex --permission-mode plan

# Validate without creating a sandbox or runtime process
ai-tester run [skill] --dry-run
ai-tester run --file ./scenario.yaml --dry-run
ai-tester run --dir ./prompts --dry-run

# Run scenarios
ai-tester run
ai-tester run <skill>
ai-tester run <skill> --scenario <scenario-id>
ai-tester run --file ./scenario.yaml --runtime codex
ai-tester run --dir ./prompts --runtime codex

# Inspect history
ai-tester history
ai-tester history <skill> --scenario <scenario-id> --last 10
ai-tester history --json

# Runtime and housekeeping
ai-tester runtimes
ai-tester sandbox-prune
ai-tester sandbox-prune --yes --min-age 300
```

Interactive runs use colorized output when stdout is a terminal. Set `NO_COLOR=1` to disable colors, or `AI_TESTER_FORCE_COLOR=1` to force ANSI colors in captured logs.

Exit codes:

- `0`: all scenarios passed
- `1`: assertion failure
- `2`: runtime, sandbox, or configuration error

## Scenario Sources

A scenario declares exactly one of:

| Field | Use for |
| --- | --- |
| `skill: <name>` | Test a skill loaded from `skills_dir`. |
| `system_prompt: |` | Test a raw inline system prompt. |
| `system_prompt_file: <path>` | Load the prompt body from a file relative to the scenario YAML. |

### Inline Prompt Example

```yaml
scenario: inline-demo
system_prompt: |
  You are a concise coding assistant.
argument: "Say done."

runner:
  runtime: codex
  model: gpt-5-codex
  permission_mode: bypassPermissions

assertions:
  - id: says-done
    type: output_contains
    pattern: "\\bdone\\b"
```

### Skill Scenario Example

```yaml
scenario: basic-skill-run
skill: demo-skill
argument: "Inspect this repo and summarize it."
max_turns: 12

runner:
  runtime: claude
  model: claude-sonnet-4-6
  permission_mode: bypassPermissions

fixtures:
  git_init: true
  files_committed:
    - path: README.md
      content: "# Demo\n"

assertions:
  - id: mentions-demo
    type: output_contains
    pattern: "(?i)demo"
  - id: stay-in-sandbox
    type: no_path_escape
```

## Scripted User Turns

By default, `ai-tester` generates the first user message from the scenario. Override it with:

```yaml
user_prompt: "/demo-skill inspect src"
```

Or run a sequence in one logical session:

```yaml
user_prompts:
  - "Study the repository. Do not edit files."
  - "Now run the actual task."
```

`user_prompt` and `user_prompts` are mutually exclusive.

## Fixtures

Fixtures create the sandbox state before the runtime starts.

```yaml
fixtures:
  git_init: true
  git_branch: feature/demo

  copy_trees:
    - from: ./fixtures/repo
      to: .

  files_committed:
    - path: README.md
      content: "# Demo\n"
    - path: src/lib.rs
      content_from: ./fixtures/lib.rs

  files_staged:
    - path: src/new.rs
      content: "pub fn new() {}\n"

  files_unstaged:
    - path: TODO.md
      content: "- audit\n"

  setup_commands:
    - git tag v0.1.0

  env:
    MY_FLAG: "1"
```

Order of operations:

1. Create temp sandbox.
2. Install the skill under `.claude/skills/<name>/` for skill-backed scenarios.
3. `git init` if requested.
4. Copy directory trees.
5. Write and commit `files_committed`.
6. Checkout `git_branch` if set.
7. Write and stage `files_staged`.
8. Write `files_unstaged`.
9. Run `setup_commands`.

`setup_commands` run through `cmd /C` on Windows and `/bin/sh -c` on Unix.

## Assertions

### `tool_called`

```yaml
- id: calls-git-status
  type: tool_called
  tool: Bash
  args_match:
    command: "^git status"

- id: calls-codegraph
  type: tool_called
  tool_pattern: "^mcp__.*__codegraph_context$"
```

### `tool_call_sequence`

```yaml
- id: status-before-commit
  type: tool_call_sequence
  sequence:
    - tool: Bash
      args_match:
        command: "^git status"
    - tool: Bash
      args_match:
        command: "^git commit"
```

### `no_tool_called`

```yaml
- id: no-web-search
  type: no_tool_called
  tool: WebSearch

- id: no-mcp-tools
  type: no_tool_called
  tool_pattern: "^mcp__"
```

### `output_contains`

```yaml
- id: mentions-result
  type: output_contains
  pattern: "(?i)done"
```

### `no_output_contains`

```yaml
- id: no-warning
  type: no_output_contains
  pattern: "WARN \\[\\+check\\]"
```

### `file_read`

Runtime-neutral check that a file was actually inspected. It matches Claude `Read(file_path)` and Codex `Bash(command)` reader commands such as `sed`, `cat`, `nl`, `rg`, `grep`, `head`, and `tail`.

```yaml
- id: reads-runtime
  type: file_read
  path: "src/runtime/mod\\.rs"
```

### `turn_count_at_most`

```yaml
- id: efficient
  type: turn_count_at_most
  max: 8
```

### `no_path_escape`

```yaml
- id: stay-in-sandbox
  type: no_path_escape
```

Implicit assertions:

- `no_unanswered_questions`: every supported question tool call must have a delivered answer. The current Claude subprocess adapter cannot deliver `user_responses` interactively, so Claude `AskUserQuestion`/`Questions` calls are treated as unanswered instead of being matched after the process exits.
- `token_budget`: emitted when a scenario or skill declares a token budget.

## Regex Semantics

`args_match`, `match_question`, `output_contains`, and `no_output_contains` support leading inline flags:

- `(?i)` case-insensitive
- `(?m)` multi-line
- `(?s)` dot matches newline

Example:

```yaml
pattern: "(?is)hello.*world"
```

## Runtime Adapters

The Rust rewrite uses external CLIs and parses JSONL output. It does not embed the previous Node SDKs.

### Codex

The Codex adapter runs:

```bash
codex exec --json --skip-git-repo-check --cd <sandbox> --sandbox <sandbox-mode>
```

Permission mapping:

| Scenario `permission_mode` | Codex args |
| --- | --- |
| `bypassPermissions` | `--dangerously-bypass-approvals-and-sandbox` |
| `acceptEdits` | `--sandbox workspace-write` |
| `plan` | `--sandbox read-only` |
| `default` | `--sandbox workspace-write` |

For `user_prompts`, later turns use `codex exec resume`.

### Claude

The Claude adapter runs:

```bash
claude -p --output-format stream-json --verbose --include-partial-messages
```

Claude receives the scenario `permission_mode` directly.

## Traces

Live runs write schema `2.0.0` traces to:

```text
runs/<skill-or-inline>/<run-id>.json
```

Trace records include:

- skill metadata and source hashes
- scenario metadata
- runner timing, model, permission mode, max turns, and sandbox path
- normalized turns and tool calls
- final output
- assertion results and weighted score
- token usage and cost when reported by the runtime
- runtime errors and parser diagnostics

## History

```bash
ai-tester history
ai-tester history --json
```

History reads v2 traces under `runs/` and prints newest runs first.

## Security Checks

Useful assertion patterns:

```yaml
- id: no-web-search
  type: no_tool_called
  tool: WebSearch

- id: no-network-shell
  type: no_tool_called
  tool: Bash
  args_match:
    command: "(?i)(curl|wget|nc|ssh|scp|rsync|ftp|telnet)|https?://|git\\s+push"

- id: no-destructive-shell
  type: no_tool_called
  tool: Bash
  args_match:
    command: "rm\\s+-[rf]+\\s+/|git\\s+push\\s+.*--force|chmod\\s+777"

- id: stay-in-sandbox
  type: no_path_escape
```

## Development

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

The test suite uses fake runtime executables and golden JSONL-style fixtures. CI must not call real model providers.

See [CONTRIBUTING.md](./CONTRIBUTING.md) for development and release details.

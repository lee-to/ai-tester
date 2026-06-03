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
- **Multi-runtime.** Built-in adapters for Claude Code and OpenAI Codex through their installed CLIs, plus generic ACP agents configured per project.
- **Three prompt sources.** Test a packaged skill, an inline `system_prompt`, or an external prompt file.
- **Scripted user turns.** Use `user_prompt` or `user_prompts` for custom session flow.
- **Declarative assertions.** `tool_called`, `tool_call_sequence`, `no_tool_called`, `output_contains`, `no_output_contains`, `file_read`, `turn_count_at_most`, and `no_path_escape`.
- **Fixtures.** Inline files, `content_from`, directory trees, staged changes, committed baselines, and setup commands.
- **Trace output.** Every live run writes a schema `2.0.0` JSON trace under `runs/`.
- **History view.** `ai-tester history` summarizes prior v2 traces.
- **Self-update.** `ai-tester update` pulls the latest GitHub release for your platform.
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

Once installed, upgrade in place with the built-in updater:

```bash
ai-tester update
```

Requires Rust **1.82 or newer** when building from source. The MSRV is fixed in
`Cargo.toml` with `rust-version = "1.82"` and checked by a dedicated Rust 1.82
CI job.

## Prerequisites

Per runtime you plan to use:

- **Claude** (`runtime: claude`, default): `claude` CLI installed and logged in.
- **Codex** (`runtime: codex`): `codex` CLI installed and logged in.
- **ACP** (`runtime: acp`): a built-in ACP profile or configured ACP agent command in `.ai-tester.yaml`.

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

# Run deterministic benchmark packs
ai-tester benchmark --suite benchmarks/js-v1/suite.yaml --runtime codex --model gpt-5-codex
ai-tester benchmark --suite benchmarks/python-v1/suite.yaml --runtime acp --agent gemini --format json
```

## Project Config

`ai-tester` walks upward from the current working directory looking for `.ai-tester.yaml`. If none is found, it falls back to `./skills` in the current directory.

```yaml
skills_dir: ./skills
defaults:
  model: claude-sonnet-4-6
  permission_mode: bypassPermissions
  setup_timeout_seconds: 60
  acp_turn_timeout_seconds: 300
  # Optional for ACP:
  # runtime: acp
  # agent: gemini
  # mode: plan
  # reasoning: high

# Optional ACP agent registry for manual overrides/custom agents:
# acp_agents:
#   gemini:
#     command: gemini
#     args: ["--experimental-acp"]
#     env: {}

# Optional MCP registry forwarded to ACP sessions:
# mcp_servers:
#   codegraph:
#     command: mock-codegraph
#     args: ["--fixture", "graph.json"]
#     env:
#       API_TOKEN: secret
#   docs:
#     type: http
#     url: http://127.0.0.1:3001/mcp
#     headers:
#       Authorization: Bearer secret
# mcp_profiles:
#   mock:
#     servers: [codegraph]
#   full:
#     servers: [codegraph, docs]
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
ai-tester run --file ./scenario.yaml --runtime acp --agent gemini
ai-tester run --file ./scenario.yaml --runtime acp --agent gemini --model gpt-5-codex --mode plan --reasoning high
ai-tester run --file ./scenario.yaml --runtime acp --agent gemini --mcp-profile full
ai-tester run --file ./scenario.yaml --runtime acp --agent gemini --acp-log ./acp-logs
ai-tester run --file ./scenario.yaml --runtime acp --agent gemini --acp-turn-timeout 120
ai-tester run --file ./scenario.yaml --setup-timeout 10
ai-tester run --dir ./prompts --runtime codex

# Choose output format (default: live events + summary)
ai-tester run <skill> --format json
ai-tester run <skill> --format markdown

# Inspect history
ai-tester history
ai-tester history <skill> --scenario <scenario-id> --last 10
ai-tester history --json
ai-tester trend <skill> --scenario <scenario-id> --last 20
ai-tester trend <skill> --json
ai-tester trace <run-id>
ai-tester trace <run-id> --json
ai-tester compare <run-a> <run-b>
ai-tester compare <run-a> <run-b> --json
ai-tester benchmark --suite benchmarks/js-v1/suite.yaml --runtime codex
ai-tester benchmark --suite benchmarks/python-v1/suite.yaml --runtime codex --format markdown

# Runtime and housekeeping
ai-tester runtimes
ai-tester sandbox-prune
ai-tester sandbox-prune --yes --min-age 300

# Self-update from the latest GitHub release
ai-tester update
ai-tester update --check
ai-tester update --force
ai-tester update --tag v1.1.0
```

### `run --format`

By default `run` streams live runtime events followed by a colored summary
(`--format live`). For CI or scripted pipelines, choose a machine-readable
format instead:

- `--format json` — suppresses live events and prints a single JSON document
  with a `summary` object (`total`, `passed`, `failed`, `errors`, `overallPass`)
  and a `runs` array of full trace records.
- `--format markdown` — prints a Markdown report: a summary line, a results
  table (scenario, skill, runtime, result, score, turns, duration), and a
  detail section per scenario that has failed assertions or errors.

Traces are still written to the `runs/` directory and exit codes are unchanged
(`0` pass, `1` assertion failures, `2` runtime errors) in every format.

```bash
ai-tester run <skill> --format json > report.json
ai-tester run <skill> --format markdown > report.md
```

### `benchmark`

`benchmark` runs a suite manifest, executes each listed scenario through the
normal `run` pipeline, and computes a deterministic `0..100` score. The score is
based on assertion correctness first, then efficiency from wall-clock time,
token usage, and tool-call count. Failed `no_path_escape` assertions force a
scenario score of `0`; failed `no_tool_called` assertions cap the scenario at
`40`.

```bash
ai-tester benchmark --suite benchmarks/js-v1/suite.yaml --runtime codex --model gpt-5-codex
ai-tester benchmark --suite benchmarks/python-v1/suite.yaml --runtime acp --agent gemini --format json
```

Suite manifests are YAML:

```yaml
suite: js-v1
version: 1
category: coding
requirements:
  commands:
    - node --version
scoring:
  correctness_weight: 0.8
  efficiency_weight: 0.2
scenarios:
  - file: tasks/01-config-precedence.yaml
    weight: 1
    time_budget_ms: 90000
    token_budget: 12000
    tool_budget: 25
```

The repository includes optional `js-v1` and `python-v1` packs under
`benchmarks/`. They use only `node` or `python3` plus standard libraries, so the
benchmark does not depend on network package installation. If a required command
is missing, the suite is reported as skipped. Set `category` to label shared
suites, for example `coding`, `frontend`, `art`, or `reasoning`; the category is
included in live, JSON, and Markdown benchmark output.

### `sandbox-prune`

Normal runs clean their sandbox automatically when the scenario scope exits.
`sandbox-prune` is housekeeping for orphan `ai-tester-*` sandbox directories left
behind by process crashes/aborts or by explicit `--keep-sandbox`. It runs as a
**dry run by default** — it lists what would be deleted; pass `--yes` to actually
remove them. Use `--min-age <seconds>` (default `60`) to only prune sandboxes
older than the given age, so active runs are never touched.

```bash
ai-tester sandbox-prune              # dry run: list orphans
ai-tester sandbox-prune --yes        # delete orphans older than 60s
ai-tester sandbox-prune --yes --min-age 300
```

### `update`

Updates the running `ai-tester` binary to the latest GitHub release for the
current platform. It detects your target triple, downloads the matching release
asset, verifies it against `SHA256SUMS.txt` when present, and replaces the binary
in place (`ai-tester --version` reports the build it resolved). Requires `curl`
and `tar`/`unzip` on `PATH`.

```bash
ai-tester update           # install the latest release if newer
ai-tester update --check   # report whether an update is available, install nothing
ai-tester update --force   # reinstall even if already current
ai-tester update --tag v1.1.0   # pin a specific release tag
```

If the binary lives in a directory you can't write to (e.g. a system path), run
the command with elevated permissions (`sudo`).

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

ACP scenarios select a configured agent by name:

```yaml
runner:
  runtime: acp
  agent: gemini
  model: gpt-5-codex
  mode: plan
  reasoning: high
  permission_mode: bypassPermissions
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
  setup_timeout_seconds: 30

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

`git_branch` and `files_staged` require `git_init: true`; scenarios that set
those fields without git initialization are rejected during dry-run validation.

`setup_commands` run through `cmd /C` on Windows and `/bin/sh -c` on Unix.
Each setup command has its own timeout. The default is 60 seconds; configure it
with `defaults.setup_timeout_seconds`, override it per scenario with
`fixtures.setup_timeout_seconds`, or override both with
`ai-tester run --setup-timeout <seconds>`. Precedence is CLI > scenario fixtures
> project defaults > 60. Values must be positive. On timeout, `ai-tester` kills
the whole setup process tree and reports the command plus stdout/stderr previews.

`fixtures.env` is scenario-scoped: it is applied to setup commands and runtime
subprocesses for Claude, Codex, and ACP. Precedence is predictable:

- Setup, Claude, and Codex: host env < `fixtures.env`.
- ACP agent process: host env < `fixtures.env` < `acp_agents.<name>.env`.
- ACP terminal bridge: host env < `fixtures.env` < `terminal/create.env`.
- MCP server env/header config is forwarded through ACP session config and is not merged with `fixtures.env`.

## Assertions

Assertion `id` values must be non-empty and unique within a scenario. Assertion
`weight` values must be finite and positive, and `turn_count_at_most.max` must
be positive. Invalid assertion specs are rejected during dry-run validation.

### `tool_called`

```yaml
- id: calls-git-status
  type: tool_called
  tool: Bash
  args_match:
    command: "^git status"
  capture: [command]
  capture_max_chars: 200

- id: calls-codegraph
  type: tool_called
  tool_pattern: "^mcp__.*__codegraph_context$"

- id: acp-runs-tests
  type: tool_called
  tool_kind: execute
  title_pattern: "Run tests"
  raw_input_match:
    command: "cargo test"
```

`tool_called` and `no_tool_called` must declare exactly one primary selector:
`tool`, `tool_pattern`, or ACP-oriented `tool_kind`. `title_pattern` filters
ACP calls by `_acpTitle`. `raw_input_match` matches ACP raw input fields: when
the trace input contains `rawInput`, paths are resolved under it; otherwise they
match the flattened ACP input.

### `tool_call_sequence`

```yaml
- id: status-before-commit
  type: tool_call_sequence
  sequence:
    - tool: Bash
      args_match:
        command: "^git status"
      capture: [command]
    - tool: Bash
      args_match:
        command: "^git commit"
      capture: [command]
    - tool_kind: execute
      title_pattern: "Run tests"
      raw_input_match:
        command: "cargo test"
  capture_max_chars: 200
```

`capture` stores top-level tool input fields from the matched tool call in the
assertion result. Captures are included in JSON traces and shown in live and
Markdown output. `capture_max_chars` truncates long captured values.

`args_match` keys can address nested trace input fields. Keys starting with `/`
use JSON Pointer, such as `/rawInput/command`. Other keys first try an exact
top-level field for backward compatibility, then dot-path lookup with numeric
array indexes, such as `rawInput.command` or `_acpLocations.0.path`. Use JSON
Pointer for field names that contain literal dots. Missing paths are treated as
an empty string, so `^$` matches an absent field and non-empty regexes do not.

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

### `file_contains` / `file_not_contains`

```yaml
- id: merge-updated
  type: file_contains
  path: merge.js
  pattern: "mergeConfig"

- id: no-secret
  type: file_not_contains
  path: output.txt
  pattern: "secret"
```

### `file_equals` / `file_exists` / `file_not_exists`

```yaml
- id: exact-output
  type: file_equals
  path: result.json
  content: "{\"ok\":true}\n"

- id: artifact-exists
  type: file_exists
  path: result.json
```

### `json_valid` / `json_path_equals`

```yaml
- id: config-valid
  type: json_valid
  path: config.json

- id: feature-enabled
  type: json_path_equals
  path: config.json
  json_path: feature.enabled
  value: true
```

`json_path` accepts the same dot-path and JSON Pointer forms as tool argument
matchers.

### `command_succeeds` / `command_output_contains`

```yaml
- id: tests-pass
  type: command_succeeds
  command: node test.js
  timeout_seconds: 10

- id: prints-ok
  type: command_output_contains
  command: python3 test.py
  pattern: "\\bok\\b"
```

Commands run inside the scenario sandbox after the model finishes and before the
sandbox is cleaned up. They default to a 30 second timeout.

### `file_read`

Runtime-neutral check that a file was actually inspected. It matches Claude
`Read(file_path)`, Codex `Bash(command)` reader commands such as `sed`, `cat`,
`nl`, `rg`, `grep`, `head`, and `tail`, and ACP `read` calls with `path`,
`file_path`, `rawInput.path`, `rawInput.file_path`, or `_acpLocations` path/URI
metadata. ACP `execute` calls use the same reader-command detection as Bash.

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

### ACP

ACP includes built-in compatibility profiles for `gemini`, `zed-claude`, and `zed-codex`.
They run through `npx` using the upstream ACP helper commands, so a minimal ACP config does
not need an `acp_agents` block:

```yaml
defaults:
  runtime: acp
  agent: gemini
```

Manual `acp_agents` entries are still supported and override a built-in with the same name:

```yaml
defaults:
  runtime: acp
  agent: gemini

acp_agents:
  gemini:
    command: ./scripts/local-gemini-acp
    args: ["--experimental-acp"]
    env: {}
```

MCP servers can be forwarded to ACP sessions:

```yaml
mcp_servers:
  codegraph:
    command: mock-codegraph
    args: ["--fixture", "graph.json"]
    env:
      API_TOKEN: secret
  docs:
    type: http
    url: http://127.0.0.1:3001/mcp
    headers:
      Authorization: Bearer secret

mcp_profiles:
  mock:
    servers: [codegraph]
  full:
    servers: [codegraph, docs]
```

`ai-tester init --acp-agent gemini` creates a minimal built-in ACP template. `ai-tester runtimes` shows both configured ACP agents and the built-in profiles with their resolved commands. Built-ins inherit the current process environment; use a manual `acp_agents` override when a profile needs explicit env values or a pinned command.

Scenario `runner.agent` or `ai-tester run --agent <name>` chooses the ACP agent. `defaults.mcp_profile`, scenario `runner.mcp_profile`, or `ai-tester run --mcp-profile <name>` chooses a profile from `mcp_profiles`; CLI has highest precedence. Scenario-level `mcp_servers` may override or add servers for a single run. The ACP runtime sends `initialize` with protocol version `1`, creates one session with the sandbox as `cwd`, forwards the effective MCP servers in `session/new.mcpServers`, applies requested ACP model/mode/reasoning config when the agent exposes compatible session options, then sends each scripted user prompt through that session.

Built-in auth requirements come from the underlying agent CLIs: Gemini supports Gemini CLI auth or `GEMINI_API_KEY` ([Gemini CLI auth docs](https://google-gemini.github.io/gemini-cli/docs/get-started/authentication.html)); `zed-claude` uses Claude Code auth or Anthropic credentials such as `ANTHROPIC_API_KEY` ([Claude Code auth docs](https://code.claude.com/docs/en/authentication)); `zed-codex` uses Codex/OpenAI credentials ([Codex CLI sign-in docs](https://help.openai.com/en/articles/11381614-api-codex-cli-and-sign-in-with-chatgpt)).

ACP model/mode negotiation uses `runner.model`, `runner.mode`, and `runner.reasoning`, with CLI flags `--model`, `--mode`, and `--reasoning` taking precedence. `runner.mode` is an ACP session mode/config selector and is separate from `permission_mode`. Explicit values from CLI, scenario YAML, or project defaults fail fast when the agent does not expose a matching option or value; the built-in model default is not forced onto ACP agents that do not advertise model selection. Successful ACP traces include an `ACP effective config` diagnostic with the applied model/mode/reasoning.

ACP prompt turns also have a wall-clock timeout separate from idle protocol progress. The default is 300 seconds; configure it with `defaults.acp_turn_timeout_seconds`, override it per scenario with `runner.acp_turn_timeout_seconds`, or override both with `ai-tester run --acp-turn-timeout <seconds>`. Precedence is CLI > scenario runner > project defaults > 300, and values must be positive. On timeout, `ai-tester` records `runner.stoppedReason` as `timeout` or `cancelled`, sends `session/cancel`, attempts `session/close`, and tears down the managed ACP process tree.

Supported MCP transports are stdio (default when `type` is omitted), `http`, and `sse`. Stdio servers use `command`, optional `args`, and optional `env`; HTTP/SSE servers use `url` and optional `headers`. Env and header values are redacted in ai-tester trace diagnostics.

For ACP protocol debugging, pass `--acp-log <dir>`. The path is treated as a directory relative to the current working directory unless absolute. Each ACP scenario writes one redacted JSONL transcript with raw stdin/stdout/stderr lines captured through the ACP transport debug hook. Protocol errors print the transcript path in live output so incompatible agents can be inspected without leaking configured env/header secrets.

ACP traces count one assistant turn per scripted user prompt sent to the ACP session. This differs from the Claude and Codex adapters, which derive turns from their runtime event streams. As a result, `turn_count_at_most` and explicit `max_turns` limits are comparable within ACP runs but not strictly identical across runtimes.

ACP tool calls are normalized using `toolCall.kind` as the trace tool name, such as `execute`, `read`, or `edit`. The trace input contains `rawInput` fields plus `_acpTitle`, `_acpKind`, `_acpStatus`, `_acpLocations`, and `_acpRawOutput` metadata when provided by the agent.

Permission requests are answered automatically:

| Scenario `permission_mode` | ACP behavior |
| --- | --- |
| `bypassPermissions`, `allow` | Select an allow option, or the first option if no allow option is labelled. |
| `plan`, `deny` | Select a reject option, or cancel if none exists. |
| `acceptEdits` | Allow only when resolved allowed-tool regexes (scenario `allowed_tools_override`, otherwise skill `allowed-tools`) match the ACP kind, title, or raw input; otherwise reject. |

`user_responses` can override the policy by matching the permission text and choosing an option by `optionId`, name, or kind.

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
- runner timing, model, optional ACP mode/reasoning, permission mode, max turns, and sandbox path
- normalized turns and tool calls
- final output
- assertion results and weighted score
- token usage and cost when reported by the runtime
- runtime errors and parser diagnostics

## History

```bash
ai-tester history
ai-tester history --json
ai-tester trend <skill> --scenario <scenario-id> --last 10
ai-tester trace <run-id>
ai-tester compare <run-a> <run-b>
```

History reads v2 traces under `runs/` and prints newest runs first.

`trend` reads v2 traces for one skill and prints the latest matching runs in chronological order. Use `--scenario` to narrow the series, `--last <n>` to cap the latest runs, and `--json` for machine-readable summaries.

`trace` pretty-prints one recorded run by `run_id`, JSON filename stem, or file path. Human output summarizes metadata, assertions, tool-call counts, turn timeline, errors, and a final-output preview; `--json` emits the full trace record.

`compare` diffs two recorded runs by `run_id`, JSON filename stem, or file path. Human output shows status, score, duration, turns, token, assertion, tool-call, and error deltas; `--json` emits the same comparison data as JSON.

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

# Roadmap

This document tracks planned CLI capabilities that are not exposed until they are useful enough to ship.

## Run Analytics Commands

The project should eventually provide first-class tools for inspecting previous runs, comparing behavior, and debugging trace output. These commands should build on the existing schema `2.0.0` JSON traces written under `runs/`.

### `ai-tester trend`

Goal: show score and reliability trends across historical runs.

Planned behavior:

- Read prior trace files from `runs/`.
- Filter by skill and optionally by scenario.
- Sort runs chronologically and show the most recent N entries.
- Display pass rate, weighted score, assertion failures, token usage, cost, turn count, and tool-call totals.
- Highlight regressions between recent runs and earlier baselines.
- Support machine-readable JSON output for automation.

Ready when:

- The command handles missing, invalid, and mixed-schema trace files gracefully.
- Output is useful in CI logs without requiring a TTY.
- Tests cover filtering, ordering, malformed traces, and aggregate metrics.

### `ai-tester compare`

Goal: compare two runs side by side and explain what changed.

Planned behavior:

- Accept two run identifiers or trace paths.
- Compare runner metadata, scenario metadata, scores, assertions, token usage, cost, turns, final output, and tool-call summaries.
- Show assertion-level differences first, because those are usually the highest-signal changes.
- Show tool-call sequence differences in a compact form.
- Support JSON output for downstream tooling.

Ready when:

- The command can resolve run IDs consistently from the `runs/` directory.
- Differences are grouped by impact rather than raw JSON order.
- Tests cover same-run comparisons, pass-to-fail changes, fail-to-pass changes, and incompatible traces.

### `ai-tester trace`

Goal: provide a readable trace viewer for a single run.

Planned behavior:

- Accept a run identifier or trace path.
- Print scenario metadata, runner settings, result summary, assertion results, cost, token usage, and turn count.
- Render tool calls in chronological order with concise inputs and outputs.
- Allow optional expanded output for debugging large tool payloads.
- Support JSON passthrough or selected-field output for scripts.

Ready when:

- The viewer makes a raw trace understandable without opening the JSON file manually.
- Large traces remain readable through truncation and explicit expansion flags.
- Tests cover path resolution, truncation, missing fields, and schema compatibility.


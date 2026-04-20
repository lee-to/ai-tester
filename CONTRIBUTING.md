# Contributing to ai-tester

Thanks for your interest in contributing. This document covers the practical bits of working on the codebase.

## Ground rules

- Be kind. See the [Code of Conduct](./CODE_OF_CONDUCT.md).
- For security issues, follow the [Security Policy](./SECURITY.md) — **do not** open a public issue.
- Open an issue before starting any non-trivial change so we can agree on the approach.

## Development setup

Requirements:

- Node.js `>= 18`
- A logged-in runtime CLI (`claude` and/or `codex`) if you plan to run actual scenarios. Pure unit work (scenario parsing, assertion logic, sandbox setup) does not need a runtime.

Clone and bootstrap:

```bash
git clone https://github.com/lee-to/ai-tester.git
cd ai-tester
npm install
npm run build
```

The CLI is runnable via `./bin/ai-tester.js`. `npm run watch` rebuilds TypeScript on save.

## Tests

```bash
npm run smoke        # synthetic-trace self-check of the assertion evaluators (no SDK, no sandbox)
```

`npm run smoke` runs in CI and must pass on every PR.

For integration coverage, add a scenario under `skills/<skill>/tests/` or a standalone YAML runnable via `--file`.

## Project layout

```
bin/           # CLI entry (thin wrapper over dist/cli.js)
src/
  cli.ts              # commander setup
  commands/           # subcommand implementations
  config/             # project config loader (.ai-tester.yaml)
  scenario/           # YAML scenario schema + loader
  sandbox/            # temp git worktree + cleanup
  runtimes/           # pluggable adapters (claude, codex, ...)
  runner/             # Claude Agent SDK integration
  assertions/         # declarative assertion evaluators
  scoring/, trace/, report/
scripts/       # dev scripts (smoke)
dist/          # tsc output (gitignored, shipped to npm)
```

## Adding a new runtime adapter

See the "Runtimes" section of [README.md](./README.md#runtimes). In short: create `src/runtimes/<name>/index.ts` that exports a `create<Name>Runtime()` factory conforming to `RuntimeAdapter`, then register it in `src/runtimes/index.ts::bootstrapRuntimes()`.

## Adding a new assertion type

1. Add a discriminated-union variant to `AssertionSchema` in `src/scenario/schema.ts`.
2. Implement the evaluator under `src/assertions/`.
3. Wire it into `evaluateAssertions` in `src/assertions/index.ts`.
4. Add a synthetic case to `scripts/smoke.mjs` that exercises both pass and fail paths.
5. Document the new assertion in README.

## Pull request checklist

- [ ] `npm run build` passes (no TypeScript errors).
- [ ] `npm run smoke` passes.
- [ ] New / changed behavior is reflected in `README.md`.
- [ ] Entry added to `CHANGELOG.md` under `## [Unreleased]`.
- [ ] Commit messages are descriptive (conventional commits preferred but not required).

## Release process

Maintainers only:

1. Update `CHANGELOG.md` — move `[Unreleased]` entries to a new version heading.
2. Bump `version` in `package.json`.
3. Tag the commit (`git tag v0.1.1 && git push --tags`).
4. `npm publish --access public` (the `prepublishOnly` script runs `clean` + `build` + `smoke`).
5. Draft a GitHub release with the changelog excerpt.

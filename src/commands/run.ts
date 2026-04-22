import chalk from "chalk";
import { createHash } from "node:crypto";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import path from "node:path";
import { loadSkill } from "../skill/discover.js";
import { listSkillNames } from "../fs/paths.js";
import {
  loadScenariosForSkill,
  loadScenarioFile,
  filterScenarios,
  resolveInlineSystemPrompt,
} from "../scenario/loader.js";
import type {
  RunOptions,
  SkillRecord,
  LoadedScenario,
  TraceRecord,
} from "../types.js";
import { bootstrapRuntimes, getRuntime, listRuntimes } from "../runtimes/index.js";
import { createSandbox, type SkillInstall } from "../sandbox/worktree.js";
import { trackCleanup } from "../sandbox/cleanup-registry.js";
import { buildTraceRecord } from "../trace/record.js";
import { writeTrace } from "../trace/writer.js";
import { evaluateAssertions } from "../assertions/index.js";
import { computeWeightedScore } from "../scoring/weighted.js";
import {
  printScenarioResult,
  printFinalSummary,
  printProgressEvent,
} from "../report/console.js";

const exec = promisify(execFile);

export async function runCommand(opts: RunOptions): Promise<number> {
  bootstrapRuntimes();
  if (opts.dryRun) {
    return runDryRun(opts);
  }

  const scenariosToRun = await discoverScenarios(opts);
  if (scenariosToRun.length === 0) {
    console.log(chalk.yellow("No scenarios matched — nothing to run."));
    return 0;
  }

  const startOfRun = Date.now();
  let scenariosRun = 0;
  let passed = 0;
  let failed = 0;
  let runtimeErrors = 0;
  let totalUsd = 0;
  const totalTokens = {
    input: 0,
    output: 0,
    cacheCreation: 0,
    cacheRead: 0,
  };

  console.log(chalk.bold("=== ai-tester ==="));
  console.log();

  for (const loaded of scenariosToRun) {
    const { scenario, filePath } = loaded;
    scenariosRun++;

    let skill: SkillRecord;
    try {
      skill = await resolveSkill(loaded);
    } catch (err) {
      console.log(chalk.red(`✗ ${scenario.scenario}: ${(err as Error).message}`));
      failed++;
      continue;
    }

    const runtimeName = opts.runtime ?? scenario.runner.runtime;
    let runtime;
    try {
      runtime = getRuntime(runtimeName);
    } catch (err) {
      console.log(chalk.red(`✗ ${scenario.scenario}: ${(err as Error).message}`));
      failed++;
      continue;
    }

    const preflight = await runtime.preflight();
    if (!preflight.ok) {
      console.log(chalk.red(`✗ ${scenario.scenario}: [${runtime.name}] ${preflight.message}`));
      runtimeErrors++;
      continue;
    }

    const effectiveScenario = opts.model
      ? { ...scenario, runner: { ...scenario.runner, model: opts.model } }
      : scenario;

    console.log(chalk.bold(`  ▶ ${scenario.scenario}`));
    console.log(
      chalk.dim(
        `    source: ${
          scenario.skill ? `skill ${skill.name}` : "inline system_prompt"
        } (${shortenPath(filePath)})`
      )
    );
    console.log(chalk.dim(`    runtime: ${runtime.name}`));

    const startedAt = new Date();
    let record: TraceRecord | null = null;
    let tracePath = "";
    let cleanup: (() => Promise<void>) | null = null;
    let untrack: (() => void) | null = null;

    try {
      const skillInstall: SkillInstall | undefined =
        scenario.skill && skill.dirPath
          ? { name: skill.name, dirPath: skill.dirPath }
          : undefined;

      const sandbox = await createSandbox(scenario.scenario, effectiveScenario.fixtures, {
        keep: opts.keepSandbox,
        skill: skillInstall,
      });
      cleanup = sandbox.cleanup;
      if (!opts.keepSandbox) untrack = trackCleanup(sandbox.cleanup);

      console.log(chalk.dim(`    sandbox: ${sandbox.path}`));
      if (sandbox.skillInstallPath) {
        console.log(chalk.dim(`    skill installed at: ${sandbox.skillInstallPath}/`));
      }

      const firstUserMessage = buildFirstUserMessage(loaded, skill);
      if (!opts.quiet) {
        console.log(chalk.dim(`    prompt: "${firstUserMessage.replaceAll("\n", " | ")}"`));
      }

      const loop = await runtime.run({
        skill,
        scenario: effectiveScenario,
        cwd: sandbox.path,
        firstUserMessage,
        skillInstallRelPath: sandbox.skillInstallPath,
        userResponses: effectiveScenario.user_responses,
        onProgress: opts.quiet ? undefined : printProgressEvent,
        idleWarnSeconds: opts.idleWarnSeconds,
      });
      const finishedAt = new Date();

      record = buildTraceRecord({
        skill,
        scenario: effectiveScenario,
        scenarioPath: filePath,
        loop,
        startedAt,
        finishedAt,
        sandboxPath: sandbox.path,
        assertions: [],
      });

      if (loop.errors.length === 0) {
        const assertionResults = evaluateAssertions(effectiveScenario.assertions, record);
        record.assertions = assertionResults;
        const allPass =
          assertionResults.every((r) => r.pass) && loop.unansweredQuestions === 0;
        record.scoring.allPassed = allPass;
        record.scoring.overallPass = allPass;
        record.scoring.weightedScore = computeWeightedScore(assertionResults);
      }

      tracePath = await writeTrace(record);
      totalUsd += record.cost.usdEstimate;
      totalTokens.input += record.cost.inputTokens;
      totalTokens.output += record.cost.outputTokens;
      totalTokens.cacheCreation += record.cost.cacheCreationTokens;
      totalTokens.cacheRead += record.cost.cacheReadTokens;

      if (loop.errors.length > 0) runtimeErrors++;
      else if (record.scoring.overallPass) passed++;
      else failed++;

      printScenarioResult(record, tracePath);
    } catch (err) {
      console.log(chalk.red(`  ${scenario.scenario} .......... ✗ runtime error`));
      console.log(chalk.red(`    ${(err as Error).message}`));
      runtimeErrors++;
    } finally {
      if (untrack) untrack();
      if (cleanup) await cleanup().catch(() => {});
    }
    console.log();
  }

  printFinalSummary({
    scenarios: scenariosRun,
    passed,
    failed,
    dispatcherErrors: runtimeErrors,
    totalUsd,
    durationMs: Date.now() - startOfRun,
    tokens: totalTokens,
  });

  if (failed > 0) return 1;
  if (runtimeErrors > 0) return 2;
  return 0;
}

// ---------------------------------------------------------------------------
// Discovery & skill resolution
// ---------------------------------------------------------------------------

async function discoverScenarios(opts: RunOptions): Promise<LoadedScenario[]> {
  // Single-file mode: --file flag OR positional skill arg that looks like a path.
  if (opts.file) {
    return [await loadScenarioFile(opts.file)];
  }
  const skillNames = opts.skill ? [opts.skill] : await listSkillNames();
  const out: LoadedScenario[] = [];
  for (const name of skillNames) {
    try {
      const scenarios = await loadScenariosForSkill(name);
      for (const s of scenarios) out.push(s);
    } catch (err) {
      throw new Error(`Scenario load error for skill "${name}": ${(err as Error).message}`);
    }
  }
  const filtered = filterScenarios(out, opts.filter);
  if (opts.scenario) {
    return filtered.filter((l) => l.scenario.scenario === opts.scenario);
  }
  return filtered;
}

async function resolveSkill(loaded: LoadedScenario): Promise<SkillRecord> {
  const { scenario } = loaded;
  if (scenario.skill) {
    return loadSkill(scenario.skill);
  }
  const inline = await resolveInlineSystemPrompt(loaded);
  if (inline === null) {
    throw new Error(
      "scenario has neither `skill` nor `system_prompt`/`system_prompt_file` — schema should have rejected this"
    );
  }
  return syntheticSkillRecord(loaded, inline);
}

function syntheticSkillRecord(loaded: LoadedScenario, body: string): SkillRecord {
  const hash = createHash("sha256").update(body).digest("hex");
  const name = `inline:${loaded.scenario.scenario}`;
  return {
    name,
    dirPath: "",
    skillMdPath: loaded.filePath,
    frontmatter: {
      name,
      description: `Inline system prompt from ${loaded.filePath}`,
    },
    body,
    bodyHash: hash,
    sourceHash: hash,
    allowedTools: [],
    allowedToolsRaw: [],
  };
}

function buildFirstUserMessage(loaded: LoadedScenario, skill: SkillRecord): string {
  const argument = loaded.scenario.argument;
  const argLine = argument && argument.length > 0 ? `\nArgument: ${argument}` : "";
  if (loaded.scenario.skill) {
    return (
      `Run the ${skill.name} skill defined in your system prompt. Follow its ` +
      `instructions end-to-end against the current working directory.` +
      argLine
    );
  }
  // Inline prompt — simpler kickoff; the system prompt already holds the instructions.
  return argument && argument.length > 0 ? argument : "Begin.";
}


// ---------------------------------------------------------------------------
// Dry-run
// ---------------------------------------------------------------------------

async function runDryRun(opts: RunOptions): Promise<number> {
  console.log(chalk.bold("=== ai-tester (dry-run) ==="));
  console.log();

  let totalScenarios = 0;
  let invalid = 0;

  if (opts.file) {
    try {
      const loaded = await loadScenarioFile(opts.file);
      printScenarioDryRun(loaded);
      totalScenarios = 1;
    } catch (err) {
      console.log(chalk.red(`✗ ${opts.file}: ${(err as Error).message}`));
      invalid++;
    }
  } else {
    const skillNames = opts.skill ? [opts.skill] : await listSkillNames();
    for (const name of skillNames) {
      let skill: SkillRecord;
      try {
        skill = await loadSkill(name);
      } catch (err) {
        console.log(chalk.red(`✗ ${name}: ${(err as Error).message}`));
        console.log();
        invalid++;
        continue;
      }
      let scenarios: LoadedScenario[] = [];
      try {
        scenarios = await loadScenariosForSkill(name);
      } catch (err) {
        console.log(chalk.bold(`Skill: ${name}`));
        printSkillMeta(skill);
        console.log(chalk.red(`  ✗ Scenario load error: ${(err as Error).message}`));
        console.log();
        invalid++;
        continue;
      }
      scenarios = filterScenarios(scenarios, opts.filter);
      if (opts.scenario) {
        scenarios = scenarios.filter((s) => s.scenario.scenario === opts.scenario);
      }
      console.log(chalk.bold(`Skill: ${name}`));
      printSkillMeta(skill);
      if (scenarios.length === 0) {
        console.log(chalk.dim(`  (no scenarios for this skill)`));
        console.log();
        continue;
      }
      console.log(`  Scenarios (${scenarios.length}):`);
      for (const loaded of scenarios) printScenarioDryRun(loaded, "    ");
      console.log();
      totalScenarios += scenarios.length;
    }
  }

  console.log(chalk.bold("=== Summary ==="));
  console.log(`  Scenarios: ${totalScenarios}`);
  console.log(`  Invalid:   ${invalid}`);
  console.log();
  if (invalid > 0) {
    console.log(chalk.red("FAIL — some scenarios failed to load."));
    return 1;
  }
  console.log(chalk.green("OK — all scenarios parsed. No sandbox created, no SDK calls made."));
  return 0;
}

function printScenarioDryRun(loaded: LoadedScenario, indent: string = ""): void {
  const { scenario, filePath } = loaded;
  const rel = shortenPath(filePath);
  const source = scenario.skill
    ? `skill ${scenario.skill}`
    : scenario.system_prompt
      ? "inline system_prompt"
      : `system_prompt_file ${scenario.system_prompt_file}`;
  console.log(`${indent}${chalk.green("✓")} ${scenario.scenario} ${chalk.dim(`(${rel})`)}`);
  console.log(`${indent}    source:         ${source}`);
  console.log(`${indent}    runtime:        ${scenario.runner.runtime}`);
  console.log(`${indent}    model:          ${scenario.runner.model}`);
  console.log(`${indent}    permission:     ${scenario.runner.permission_mode}`);
  console.log(
    `${indent}    max_turns:      ${
      typeof scenario.max_turns === "number"
        ? String(scenario.max_turns)
        : chalk.dim("(auto)")
    }`
  );
  console.log(`${indent}    argument:       ${scenario.argument ?? chalk.dim("(none)")}`);
  console.log(
    `${indent}    fixtures:       ${scenario.fixtures.files_committed.length} committed, ` +
      `${scenario.fixtures.files_staged.length} staged, ` +
      `${scenario.fixtures.files_unstaged.length} unstaged`
  );
  console.log(
    `${indent}    git:            init=${scenario.fixtures.git_init} branch=${
      scenario.fixtures.git_branch ?? chalk.dim("(default)")
    }`
  );
  console.log(`${indent}    user_responses: ${scenario.user_responses.length}`);
  console.log(`${indent}    assertions:     ${scenario.assertions.length}`);
}

function printSkillMeta(skill: SkillRecord): void {
  const version = skill.frontmatter.version ?? chalk.dim("-");
  const hashShort = skill.sourceHash.slice(0, 8);
  console.log(`  Version:     ${version}`);
  console.log(`  Hash:        ${hashShort}`);
  const tools = skill.allowedTools;
  console.log(`  Allowed tools (${tools.length}):`);
  for (const t of tools) {
    const label = t.scopes.length > 0 ? `${t.name}(${t.scopes.join(", ")})` : t.name;
    console.log(`    ${label}`);
  }
}

function shortenPath(absPath: string): string {
  const cwd = process.cwd();
  if (absPath.startsWith(cwd + path.sep)) return absPath.slice(cwd.length + 1);
  return absPath;
}

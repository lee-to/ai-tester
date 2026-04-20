import path from "node:path";
import type { Scenario } from "../scenario/schema.js";
import type { SkillRecord, TraceRecord, Turn, AssertionResult } from "../types.js";
import type { SdkRunResult } from "../runner/sdk-runner.js";

export interface BuildRecordParams {
  skill: SkillRecord;
  scenario: Scenario;
  scenarioPath: string;
  loop: SdkRunResult;
  startedAt: Date;
  finishedAt: Date;
  sandboxPath: string | null;
  assertions?: AssertionResult[];
}

export function buildTraceRecord(params: BuildRecordParams): TraceRecord {
  const {
    skill,
    scenario,
    scenarioPath,
    loop,
    startedAt,
    finishedAt,
    sandboxPath,
    assertions = [],
  } = params;

  const sourceHashShort = skill.sourceHash.slice(0, 8);
  const isoCompact = finishedAt.toISOString().replaceAll(":", "-").replace(/\.\d+Z$/, "Z");
  const version = skill.frontmatter.version ?? "no-version";
  const runId = `${skill.name}__${isoCompact}__${version}__${sourceHashShort}`;

  const toolCallSummary = summarizeToolCalls(loop.turns, loop.unansweredQuestions);
  const allPassed = assertions.length > 0 ? assertions.every((a) => a.pass) : true;
  // Only treat an exhausted turn budget as a failure when the scenario
  // explicitly set `max_turns`. Hitting the internal fallback cap is a warning.
  const budgetFailure = loop.stoppedReason === "max_turns" && loop.maxTurnsUserSet;
  const overallPass =
    allPassed &&
    loop.errors.length === 0 &&
    loop.unansweredQuestions === 0 &&
    !budgetFailure;

  return {
    schemaVersion: "1.1.0",
    runId,
    skill: {
      name: skill.name,
      path: relativize(skill.skillMdPath),
      version: skill.frontmatter.version ?? null,
      sourceHash: skill.sourceHash,
      sourceHashShort,
      bodyHash: skill.bodyHash,
      allowedToolsParsed: skill.allowedTools,
      allowedToolsRaw: skill.allowedToolsRaw,
    },
    scenario: {
      name: scenario.scenario,
      path: relativize(scenarioPath),
      argument: scenario.argument ?? null,
    },
    runner: {
      model: scenario.runner.model,
      permissionMode: scenario.runner.permission_mode,
      startedAt: startedAt.toISOString(),
      finishedAt: finishedAt.toISOString(),
      durationMs: finishedAt.getTime() - startedAt.getTime(),
      maxTurns: loop.maxTurnsEffective,
      maxTurnsUserSet: loop.maxTurnsUserSet,
      turnsUsed: loop.turnsUsed,
      hitMaxTurns: loop.stoppedReason === "max_turns",
      sessionId: loop.sessionId,
      sandboxPath,
    },
    turns: loop.turns,
    finalOutput: loop.finalOutput,
    toolCallSummary,
    assertions,
    scoring: { allPassed, overallPass },
    cost: {
      inputTokens: loop.cost.inputTokens,
      outputTokens: loop.cost.outputTokens,
      cacheCreationTokens: loop.cost.cacheCreationTokens,
      cacheReadTokens: loop.cost.cacheReadTokens,
      usdEstimate: loop.cost.usdEstimate,
      source: loop.cost.usdEstimate > 0 ? "sdk" : "unknown",
    },
    errors: loop.errors,
  };
}

function summarizeToolCalls(
  turns: Turn[],
  unansweredQuestions: number
): TraceRecord["toolCallSummary"] {
  let total = 0;
  const byTool: Record<string, number> = {};
  for (const turn of turns) {
    for (const tc of turn.toolCalls ?? []) {
      total++;
      byTool[tc.name] = (byTool[tc.name] ?? 0) + 1;
    }
  }
  return { total, byTool, unansweredQuestions };
}

function relativize(absPath: string): string {
  const cwd = process.cwd();
  if (absPath === cwd) return ".";
  if (absPath.startsWith(cwd + path.sep)) return absPath.slice(cwd.length + 1);
  return absPath;
}

import path from "node:path";
import chalk from "chalk";
import type { AssertionResult, TraceRecord } from "../types.js";
import type { ProgressEvent } from "../runner/sdk-runner.js";

/** Formats a single ProgressEvent into a short, readable one-line update. */
export function printProgressEvent(event: ProgressEvent): void {
  const t = formatElapsed(getElapsed(event));
  switch (event.kind) {
    case "system_init":
      console.log(chalk.dim(`    ${t} ▸ session ${event.sessionId ?? "(no id)"}`));
      break;
    case "assistant_text": {
      const text = event.text.trim().replaceAll(/\s+/g, " ");
      const preview = truncate(text, 180);
      if (preview) console.log(chalk.dim(`    ${t} ▸ `) + chalk.italic(preview));
      break;
    }
    case "tool_use": {
      const argsPreview = formatInputPreview(event.tool, event.input);
      console.log(chalk.dim(`    ${t} ▸ `) + chalk.cyan(`${event.tool}`) + chalk.dim(` ${argsPreview}`));
      break;
    }
    case "tool_result": {
      const result = truncate(event.content.replaceAll(/\s+/g, " "), 120);
      const prefix = event.isError ? chalk.red("!err") : chalk.green("  ok");
      console.log(chalk.dim(`    ${t} ◂ `) + prefix + chalk.dim(` ${event.tool}: ${result}`));
      break;
    }
    case "question_answered":
      console.log(
        chalk.dim(`    ${t} ? `) +
          chalk.yellow(`${event.tool}`) +
          chalk.dim(` "${truncate(event.questionPreview, 60)}" → `) +
          chalk.green(event.chosen)
      );
      break;
    case "question_unanswered":
      console.log(
        chalk.dim(`    ${t} ? `) +
          chalk.yellow(`${event.tool}`) +
          chalk.dim(` "${truncate(event.questionPreview, 60)}" → `) +
          chalk.red("no matching user_responses")
      );
      break;
    case "result":
      console.log(
        chalk.dim(`    ${t} ● finished`) +
          chalk.dim(` (${event.subtype ?? "?"}) cost ~$${event.usdEstimate.toFixed(4)}`)
      );
      break;
    case "stderr": {
      const line = event.chunk.trim();
      if (!line) break;
      for (const l of line.split("\n")) {
        if (l.trim()) console.log(chalk.dim(`    ${t} [cli] `) + chalk.gray(l));
      }
      break;
    }
    case "idle_warning":
      console.log(
        chalk.yellow(
          `    … idle for ${event.secondsSinceLastEvent}s — CLI may be stuck (Ctrl-C to abort)`
        )
      );
      break;
  }
}

function getElapsed(event: ProgressEvent): number {
  return "elapsedMs" in event ? event.elapsedMs : 0;
}

function formatElapsed(ms: number): string {
  if (ms < 1000) return `${ms}ms`.padStart(5, " ");
  const s = ms / 1000;
  if (s < 60) return `${s.toFixed(1)}s`.padStart(5, " ");
  const m = Math.floor(s / 60);
  const rs = Math.round(s - m * 60);
  return `${m}m${rs.toString().padStart(2, "0")}s`;
}

function formatInputPreview(tool: string, input: Record<string, unknown>): string {
  const keyOrder: Record<string, string[]> = {
    Bash: ["command"],
    Read: ["file_path"],
    Write: ["file_path"],
    Edit: ["file_path"],
    Glob: ["pattern"],
    Grep: ["pattern"],
    AskUserQuestion: ["questions"],
    Questions: ["questions"],
    WebFetch: ["url"],
    WebSearch: ["query"],
    Skill: ["skill"],
  };
  const keys = keyOrder[tool] ?? Object.keys(input).slice(0, 2);
  const pairs: string[] = [];
  for (const k of keys) {
    const v = input[k];
    if (v === undefined) continue;
    if (tool === "AskUserQuestion" || tool === "Questions") {
      const first =
        Array.isArray(v) && v.length > 0 && v[0] && typeof v[0] === "object"
          ? String((v[0] as Record<string, unknown>).question ?? "")
          : "";
      pairs.push(`"${truncate(first, 60)}"`);
      continue;
    }
    const s = typeof v === "string" ? v : JSON.stringify(v);
    pairs.push(truncate(s, 80));
  }
  return pairs.join(" ");
}

function truncate(s: string, n: number): string {
  return s.length > n ? s.slice(0, n - 1) + "…" : s;
}

export function printScenarioResult(record: TraceRecord, tracePath: string): void {
  const overallPass = record.scoring.overallPass;
  const label = `  ${record.scenario.name} `.padEnd(46, ".");
  const mark = overallPass ? chalk.green(" ✓") : chalk.red(" ✗");
  process.stdout.write(label + mark + "\n");

  for (const a of record.assertions) {
    const m = a.pass ? chalk.green("✓") : chalk.red("✗");
    const line = `    ${m} ${a.id}`;
    const detail = a.detail ? chalk.dim(`  — ${a.detail}`) : "";
    console.log(line + detail);
  }

  // Turn-budget line: failure only when user explicitly set max_turns and we
  // hit it; otherwise show as info/warning.
  if (record.runner.hitMaxTurns) {
    if (record.runner.maxTurnsUserSet) {
      console.log(
        chalk.red(
          `    ✗ turn_budget — hit explicit max_turns=${record.runner.maxTurns} before skill finished`
        )
      );
    } else {
      console.log(
        chalk.yellow(
          `    ⚠ hit internal safety cap of ${record.runner.maxTurns} turns — add ` +
            `\`max_turns: N\` to the scenario to set an explicit budget`
        )
      );
    }
  }

  for (const err of record.errors) {
    console.log(chalk.red(`    ✗ ${err.kind}: ${err.message}`));
  }

  const cachePct = computeCachePct(record);
  const tokens = record.cost.inputTokens + record.cost.outputTokens;
  const turnsSuffix = record.runner.maxTurnsUserSet ? "" : chalk.dim(" auto-cap");
  console.log(
    chalk.dim(
      `      turns: ${record.runner.turnsUsed}/${record.runner.maxTurns}` +
        turnsSuffix +
        chalk.dim(
          `, tools: ${record.toolCallSummary.total}, ` +
            `tokens: ${tokens} (cache-read ${cachePct}%), ` +
            `cost: ~$${record.cost.usdEstimate.toFixed(4)}`
        )
    )
  );
  console.log(chalk.dim(`      trace: ${shortenPath(tracePath)}`));
}

export function printFinalSummary(params: {
  scenarios: number;
  passed: number;
  failed: number;
  dispatcherErrors: number;
  totalUsd: number;
  durationMs: number;
}): void {
  console.log(chalk.bold("=== Results ==="));
  console.log(`  Scenarios:         ${params.scenarios}`);
  console.log(`  Passed:            ${chalk.green(String(params.passed))}`);
  console.log(`  Failed:            ${params.failed > 0 ? chalk.red(String(params.failed)) : "0"}`);
  console.log(
    `  Dispatcher errors: ${params.dispatcherErrors > 0 ? chalk.red(String(params.dispatcherErrors)) : "0"}`
  );
  console.log(`  Duration:          ${(params.durationMs / 1000).toFixed(1)}s`);
  console.log(`  Estimated cost:    ~$${params.totalUsd.toFixed(4)}`);
  console.log();
  if (params.failed > 0 || params.dispatcherErrors > 0) {
    console.log(chalk.red("FAIL"));
  } else if (params.scenarios === 0) {
    console.log(chalk.yellow("NO SCENARIOS MATCHED"));
  } else {
    console.log(chalk.green("PASS"));
  }
}

function computeCachePct(record: TraceRecord): number {
  const total =
    record.cost.inputTokens +
    record.cost.cacheCreationTokens +
    record.cost.cacheReadTokens;
  if (total === 0) return 0;
  return Math.round((record.cost.cacheReadTokens / total) * 100);
}

function shortenPath(absPath: string): string {
  const cwd = process.cwd();
  if (absPath === cwd) return ".";
  if (absPath.startsWith(cwd + path.sep)) return absPath.slice(cwd.length + 1);
  return absPath;
}

// Unused export placeholder for future LLM-judge detail strings.
export function formatAssertionResultOneLine(a: AssertionResult): string {
  const mark = a.pass ? chalk.green("✓") : chalk.red("✗");
  return `${mark} ${a.id}${a.detail ? ` — ${a.detail}` : ""}`;
}

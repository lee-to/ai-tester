import fs from "node:fs/promises";
import path from "node:path";
import chalk from "chalk";
import { getRunsDir } from "../fs/paths.js";
import type { TraceRecord } from "../types.js";

export interface HistoryOptions {
  skill?: string;
  scenario?: string;
  last?: number;
  json?: boolean;
}

interface HistoryEntry {
  runId: string;
  filePath: string;
  skill: string;
  scenario: string;
  finishedAt: string;
  overallPass: boolean;
  durationMs: number;
  turnsUsed: number;
  tokensTotal: number;
  tokens: {
    input: number;
    output: number;
    cacheCreation: number;
    cacheRead: number;
  };
  usdEstimate: number;
  tokenBudget: number | null;
  budgetExceeded: boolean;
  errorCount: number;
}

export async function historyCommand(opts: HistoryOptions): Promise<number> {
  const runsDir = getRunsDir();
  let skillDirs: string[];
  try {
    const entries = await fs.readdir(runsDir, { withFileTypes: true });
    skillDirs = entries
      .filter((e) => e.isDirectory())
      .map((e) => e.name)
      .filter((name) => (opts.skill ? name === opts.skill : true));
  } catch (err) {
    if ((err as NodeJS.ErrnoException).code === "ENOENT") {
      console.log(chalk.yellow("No runs/ directory found — run some scenarios first."));
      return 0;
    }
    throw err;
  }

  const entries: HistoryEntry[] = [];
  for (const name of skillDirs) {
    const dir = path.join(runsDir, name);
    let files: string[];
    try {
      files = (await fs.readdir(dir)).filter((f) => f.endsWith(".json"));
    } catch {
      continue;
    }
    for (const f of files) {
      const full = path.join(dir, f);
      try {
        const raw = await fs.readFile(full, "utf-8");
        const rec = JSON.parse(raw) as TraceRecord;
        if (opts.scenario && rec.scenario.name !== opts.scenario) continue;
        entries.push(toEntry(rec, full));
      } catch {
        // skip unreadable / malformed traces
      }
    }
  }

  entries.sort((a, b) => (a.finishedAt < b.finishedAt ? 1 : -1));
  const limit = opts.last && opts.last > 0 ? opts.last : 20;
  const shown = entries.slice(0, limit);

  if (opts.json) {
    console.log(JSON.stringify(shown, null, 2));
    return 0;
  }

  if (shown.length === 0) {
    console.log(chalk.yellow("No runs matched."));
    return 0;
  }

  const totalFound = entries.length;
  console.log(
    chalk.bold(`=== Run history ===`) +
      chalk.dim(` (showing ${shown.length} of ${totalFound})`)
  );
  console.log();

  for (const e of shown) {
    const mark = e.overallPass ? chalk.green("✓") : chalk.red("✗");
    const label = `${e.skill}/${e.scenario}`;
    const when = formatWhen(e.finishedAt);
    const dur = formatDuration(e.durationMs);
    const tok = e.tokensTotal.toLocaleString("en-US");
    const cost = e.usdEstimate > 0 ? `~$${e.usdEstimate.toFixed(4)}` : chalk.dim("—");
    const budget = e.tokenBudget != null ? `/${e.tokenBudget.toLocaleString("en-US")}` : "";
    const budgetTag = e.budgetExceeded ? chalk.red(" over-budget") : "";
    console.log(
      `  ${mark} ${chalk.dim(when)}  ${label.padEnd(44)} ` +
        chalk.dim(`${dur.padStart(6)}  ${e.turnsUsed}t  `) +
        `${tok}${budget} tok  ${cost}${budgetTag}`
    );
    if (!e.overallPass && e.errorCount > 0) {
      console.log(chalk.dim(`      ${e.errorCount} error(s); trace: ${shortenPath(e.filePath)}`));
    }
  }

  console.log();
  const totalTokens = shown.reduce((s, e) => s + e.tokensTotal, 0);
  const totalUsd = shown.reduce((s, e) => s + e.usdEstimate, 0);
  const passed = shown.filter((e) => e.overallPass).length;
  console.log(
    chalk.dim(
      `  Σ ${shown.length} run(s), ${passed} pass, ${shown.length - passed} fail, ` +
        `${totalTokens.toLocaleString("en-US")} tokens, ~$${totalUsd.toFixed(4)}`
    )
  );

  return 0;
}

function toEntry(rec: TraceRecord, filePath: string): HistoryEntry {
  const tokens = {
    input: rec.cost.inputTokens,
    output: rec.cost.outputTokens,
    cacheCreation: rec.cost.cacheCreationTokens,
    cacheRead: rec.cost.cacheReadTokens,
  };
  const total = tokens.input + tokens.output + tokens.cacheCreation + tokens.cacheRead;
  const budget = rec.scenario.tokenBudget ?? rec.skill.tokenBudget ?? null;
  return {
    runId: rec.runId,
    filePath,
    skill: rec.skill.name,
    scenario: rec.scenario.name,
    finishedAt: rec.runner.finishedAt,
    overallPass: rec.scoring.overallPass,
    durationMs: rec.runner.durationMs,
    turnsUsed: rec.runner.turnsUsed,
    tokensTotal: total,
    tokens,
    usdEstimate: rec.cost.usdEstimate,
    tokenBudget: budget,
    budgetExceeded: budget != null && total > budget,
    errorCount: rec.errors.length,
  };
}

function formatWhen(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const pad = (n: number) => n.toString().padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  const s = ms / 1000;
  if (s < 60) return `${s.toFixed(1)}s`;
  const m = Math.floor(s / 60);
  const rs = Math.round(s - m * 60);
  return `${m}m${rs.toString().padStart(2, "0")}s`;
}

function shortenPath(absPath: string): string {
  const cwd = process.cwd();
  if (absPath.startsWith(cwd + path.sep)) return absPath.slice(cwd.length + 1);
  return absPath;
}

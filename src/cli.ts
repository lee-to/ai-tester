import { Command } from "commander";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { runCommand } from "./commands/run.js";
import { trendCommand } from "./commands/trend.js";
import { compareCommand } from "./commands/compare.js";
import { traceCommand } from "./commands/trace.js";
import { sandboxPruneCommand } from "./commands/sandbox-prune.js";

const pkg = JSON.parse(
  readFileSync(
    resolve(dirname(fileURLToPath(import.meta.url)), "../package.json"),
    "utf-8"
  )
) as { version: string };

const program = new Command();

program
  .name("ai-tester")
  .description("Behavioral test harness for Claude Code skills and bare prompts (multi-runtime)")
  .version(pkg.version);

program
  .command("run")
  .description("Run scenarios for a skill (or all skills)")
  .argument("[skill]", "Skill name to run. Omit to run every discovered skill.")
  .option("--scenario <name>", "Run a single scenario by its id")
  .option("--file <path>", "Run a single scenario file directly (bypasses skill discovery)")
  .option("--model <id>", "Override runner.model")
  .option("--runtime <name>", "Override runner.runtime (e.g. claude, codex)")
  .option("--filter <regex>", "Filter scenarios whose id matches the regex")
  .option("--dry-run", "Parse and validate scenarios, no sandbox or SDK calls")
  .option("--keep-sandbox", "Don't delete the sandbox worktree after run")
  .option("--quiet", "Hide live progress events; only show final summary")
  .option(
    "--idle-warn <seconds>",
    "Print a warning when no stream event arrives for N seconds (default 30)",
    "30"
  )
  .action(async (skill, opts) => {
    try {
      const exitCode = await runCommand({
        skill,
        scenario: opts.scenario,
        file: opts.file,
        model: opts.model,
        runtime: opts.runtime,
        filter: opts.filter,
        dryRun: opts.dryRun === true,
        keepSandbox: opts.keepSandbox === true,
        quiet: opts.quiet === true,
        idleWarnSeconds: Number.parseInt(opts.idleWarn, 10) || 30,
      });
      process.exit(exitCode);
    } catch (err) {
      console.error((err as Error).message);
      process.exit(2);
    }
  });

program
  .command("trend")
  .description("Show a score trend across recent runs (coming soon)")
  .argument("<skill>", "Skill name")
  .option("--scenario <name>", "Single scenario trend")
  .option("--last <n>", "Last N runs", "20")
  .action(async (_skill, _opts) => {
    await trendCommand();
    process.exit(0);
  });

program
  .command("compare")
  .description("Diff two runs (coming soon)")
  .argument("<runA>", "Run id A")
  .argument("<runB>", "Run id B")
  .action(async (_a, _b) => {
    await compareCommand();
    process.exit(0);
  });

program
  .command("trace")
  .description("Pretty-print a recorded trace (coming soon)")
  .argument("<runId>", "Run id")
  .action(async (_id) => {
    await traceCommand();
    process.exit(0);
  });

program
  .command("runtimes")
  .description("List available runtime adapters.")
  .action(async () => {
    const { bootstrapRuntimes, listRuntimes } = await import("./runtimes/index.js");
    bootstrapRuntimes();
    for (const rt of listRuntimes()) {
      const preflight = await rt.preflight();
      const status = preflight.ok ? "\x1b[32mready\x1b[0m" : "\x1b[33mnot-ready\x1b[0m";
      console.log(`  ${rt.name.padEnd(10)} ${status}  ${rt.description}`);
      if (!preflight.ok) console.log(`             ${preflight.message}`);
    }
    process.exit(0);
  });

program
  .command("sandbox-prune")
  .description(
    "List (and optionally delete) orphan sandbox worktrees under $TMPDIR — left behind by interrupted runs."
  )
  .option("--yes", "Actually delete the sandboxes. Without this, only lists them.")
  .option(
    "--min-age <seconds>",
    "Ignore sandboxes newer than this — default 60 so in-flight runs are safe.",
    "60"
  )
  .action(async (opts) => {
    const parsed = Number.parseInt(opts.minAge, 10);
    const exitCode = await sandboxPruneCommand({
      yes: opts.yes === true,
      minAgeSeconds: Number.isFinite(parsed) && parsed >= 0 ? parsed : 60,
    });
    process.exit(exitCode);
  });

program.parseAsync(process.argv).catch((err) => {
  console.error(err);
  process.exit(2);
});

import chalk from "chalk";

export async function traceCommand(): Promise<void> {
  console.log(
    chalk.yellow(
      "trace: not implemented in MVP. Planned for Phase 5 — will pretty-print a trace JSON."
    )
  );
}

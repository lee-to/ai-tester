import chalk from "chalk";

export async function trendCommand(): Promise<void> {
  console.log(
    chalk.yellow(
      "trend: not implemented in MVP. Planned for Phase 5 — will read runs/*.json and show score history."
    )
  );
}

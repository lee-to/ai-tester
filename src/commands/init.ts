import fs from "node:fs/promises";
import path from "node:path";
import chalk from "chalk";

export interface InitOptions {
  force?: boolean;
  skillsDir?: string;
  model?: string;
  permissionMode?: "acceptEdits" | "bypassPermissions" | "plan" | "default";
}

const CONFIG_FILENAME = ".ai-tester.yaml";

export async function initCommand(opts: InitOptions): Promise<number> {
  const cwd = process.cwd();
  const target = path.join(cwd, CONFIG_FILENAME);

  const exists = await fileExists(target);
  if (exists && !opts.force) {
    console.error(
      chalk.red(`${CONFIG_FILENAME} already exists at ${target}`) +
        chalk.dim("\n  Pass --force to overwrite.")
    );
    return 1;
  }

  const skillsDir = opts.skillsDir ?? "./skills";
  const model = opts.model ?? "claude-sonnet-4-6";
  const permissionMode = opts.permissionMode ?? "bypassPermissions";

  const body =
    `skills_dir: ${skillsDir}\n` +
    `defaults:\n` +
    `  model: ${model}\n` +
    `  permission_mode: ${permissionMode}\n`;

  await fs.writeFile(target, body, "utf-8");

  console.log(
    (exists ? chalk.yellow("Overwrote ") : chalk.green("Created ")) + target
  );
  console.log(chalk.dim(`  skills_dir:      ${skillsDir}`));
  console.log(chalk.dim(`  defaults.model:  ${model}`));
  console.log(chalk.dim(`  permission_mode: ${permissionMode}`));
  return 0;
}

async function fileExists(p: string): Promise<boolean> {
  try {
    await fs.access(p);
    return true;
  } catch {
    return false;
  }
}

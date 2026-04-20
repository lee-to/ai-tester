import fs from "node:fs/promises";
import path from "node:path";
import os from "node:os";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import type { FixtureSpec } from "../scenario/schema.js";

const exec = promisify(execFile);

export interface Sandbox {
  path: string;
  cleanup: () => Promise<void>;
  /** Relative path under the sandbox where the skill was installed, or null. */
  skillInstallPath: string | null;
}

export interface SkillInstall {
  name: string;
  dirPath: string;
}

/**
 * Create a temp sandbox directory and apply the scenario fixtures.
 * Order of operations:
 *   1. mkdtemp
 *   2. git init (if requested) + user/email config
 *   3. write + commit `files_committed` as baseline
 *   4. checkout the target branch (if given)
 *   5. write `files_staged` and `git add` them — left staged, not committed
 *   6. write `files_unstaged` — untracked/modified, not staged
 *   7. run arbitrary `setup_commands`
 */
export async function createSandbox(
  scenarioName: string,
  fixtures: FixtureSpec,
  opts: { keep?: boolean; skill?: SkillInstall } = {}
): Promise<Sandbox> {
  const raw = await fs.mkdtemp(path.join(os.tmpdir(), `ai-tester-${safeName(scenarioName)}-`));
  // Resolve through symlinks (macOS prefixes /tmp and /var with /private).
  // Having the canonical path up front means path-escape assertions compare
  // against the same form that Claude Code emits in its tool calls.
  const base = await fs.realpath(raw);
  let skillInstallPath: string | null = null;

  try {
    if (opts.skill) {
      const rel = path.join(".claude", "skills", opts.skill.name);
      const dest = path.join(base, rel);
      await fs.cp(opts.skill.dirPath, dest, { recursive: true });
      skillInstallPath = rel;
    }

    if (fixtures.git_init) {
      await runGit(base, ["init", "-q"]);
      await runGit(base, ["config", "user.email", "ai-tester@example.com"]);
      await runGit(base, ["config", "user.name", "ai-tester"]);
      await runGit(base, ["config", "commit.gpgsign", "false"]);
      // When a skill is pre-installed under .claude/skills/, we don't want it
      // to pollute git status / git diff --cached output — the skill under
      // test shouldn't see its own source tree as "project changes".
      if (opts.skill) {
        await fs.writeFile(path.join(base, ".gitignore"), ".claude/\n", "utf-8");
      }
    }

    for (const tree of fixtures.copy_trees) {
      await copyTree(base, tree.from, tree.to);
    }

    for (const file of fixtures.files_committed) {
      await writeFixture(base, file.path, file.content ?? "");
    }
    const hasBaselineContent =
      fixtures.copy_trees.length > 0 || fixtures.files_committed.length > 0;
    if (fixtures.git_init && hasBaselineContent) {
      await runGit(base, ["add", "-A"]);
      await runGit(base, ["commit", "-q", "-m", "ai-tester: fixture baseline"]);
    } else if (fixtures.git_init) {
      const marker = path.join(base, ".ai-tester-keep");
      await fs.writeFile(marker, "");
      await runGit(base, ["add", ".ai-tester-keep"]);
      await runGit(base, ["commit", "-q", "-m", "ai-tester: initial empty commit"]);
    }

    if (fixtures.git_init && fixtures.git_branch) {
      await runGit(base, ["checkout", "-q", "-B", fixtures.git_branch]);
    }

    for (const file of fixtures.files_staged) {
      await writeFixture(base, file.path, file.content ?? "");
    }
    if (fixtures.git_init && fixtures.files_staged.length > 0) {
      await runGit(base, ["add", ...fixtures.files_staged.map((f) => f.path)]);
    }

    for (const file of fixtures.files_unstaged) {
      await writeFixture(base, file.path, file.content ?? "");
    }

    for (const cmd of fixtures.setup_commands) {
      await runShell(base, cmd);
    }

    return {
      path: base,
      skillInstallPath,
      cleanup: async () => {
        if (opts.keep) return;
        await fs.rm(base, { recursive: true, force: true });
      },
    };
  } catch (err) {
    if (!opts.keep) {
      await fs.rm(base, { recursive: true, force: true }).catch(() => {});
    }
    throw new Error(`sandbox setup failed for "${scenarioName}": ${(err as Error).message}`);
  }
}

async function writeFixture(base: string, relPath: string, content: string): Promise<void> {
  const abs = path.resolve(base, relPath);
  if (!abs.startsWith(base + path.sep) && abs !== base) {
    throw new Error(`fixture path escapes sandbox: ${relPath} → ${abs}`);
  }
  await fs.mkdir(path.dirname(abs), { recursive: true });
  await fs.writeFile(abs, content, "utf-8");
}

/** Copy the CONTENTS of `absSrc` into `<base>/<relDest>`. */
async function copyTree(base: string, absSrc: string, relDest: string): Promise<void> {
  const normDest = relDest === "" || relDest === "." ? base : path.resolve(base, relDest);
  if (!normDest.startsWith(base + path.sep) && normDest !== base) {
    throw new Error(`copy_trees.to escapes sandbox: ${relDest} → ${normDest}`);
  }
  await fs.mkdir(normDest, { recursive: true });
  await fs.cp(absSrc, normDest, { recursive: true });
}

async function runGit(cwd: string, args: string[]): Promise<{ stdout: string }> {
  try {
    const { stdout } = await exec("git", args, { cwd });
    return { stdout };
  } catch (err) {
    const e = err as NodeJS.ErrnoException & { stderr?: string };
    throw new Error(`git ${args.join(" ")} failed: ${e.stderr?.trim() ?? e.message}`);
  }
}

async function runShell(cwd: string, command: string): Promise<void> {
  try {
    await exec("/bin/sh", ["-c", command], { cwd });
  } catch (err) {
    const e = err as NodeJS.ErrnoException & { stderr?: string };
    throw new Error(`setup command failed: "${command}" — ${e.stderr?.trim() ?? e.message}`);
  }
}

function safeName(s: string): string {
  return s.replace(/[^a-zA-Z0-9_-]+/g, "-").slice(0, 40);
}

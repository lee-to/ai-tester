import fs from "node:fs/promises";
import path from "node:path";
import os from "node:os";
import chalk from "chalk";

export interface SandboxPruneOptions {
  yes?: boolean;
  minAgeSeconds?: number;
}

interface OrphanRecord {
  name: string;
  path: string;
  ageSeconds: number;
  sizeBytes: number;
}

/**
 * Find and optionally remove orphaned sandbox worktrees left behind by
 * interrupted / crashed ai-tester runs. Matches the `$TMPDIR/ai-tester-*`
 * prefix. A minimum age filter (default 60s) keeps currently-running
 * sandboxes safe.
 */
export async function sandboxPruneCommand(opts: SandboxPruneOptions): Promise<number> {
  const minAgeMs = Math.max(0, (opts.minAgeSeconds ?? 60)) * 1000;
  const tmpDir = os.tmpdir();

  let entries: string[] = [];
  try {
    entries = await fs.readdir(tmpDir);
  } catch (err) {
    console.error(chalk.red(`Could not read ${tmpDir}: ${(err as Error).message}`));
    return 2;
  }

  const now = Date.now();
  const orphans: OrphanRecord[] = [];
  let skippedYoung = 0;

  for (const name of entries) {
    if (!name.startsWith("ai-tester-")) continue;
    const full = path.join(tmpDir, name);
    let st;
    try {
      st = await fs.stat(full);
    } catch {
      continue;
    }
    if (!st.isDirectory()) continue;
    const ageMs = now - st.mtimeMs;
    if (ageMs < minAgeMs) {
      skippedYoung++;
      continue;
    }
    const sizeBytes = await dirSize(full);
    orphans.push({
      name,
      path: full,
      ageSeconds: Math.round(ageMs / 1000),
      sizeBytes,
    });
  }

  orphans.sort((a, b) => b.ageSeconds - a.ageSeconds);

  if (orphans.length === 0) {
    console.log(chalk.green(`No orphan sandboxes found under ${tmpDir}`));
    if (skippedYoung > 0) {
      console.log(
        chalk.dim(
          `  ${skippedYoung} sandbox(es) skipped as younger than ${opts.minAgeSeconds ?? 60}s ` +
            `(likely in-flight runs).`
        )
      );
    }
    return 0;
  }

  const totalBytes = orphans.reduce((acc, o) => acc + o.sizeBytes, 0);

  console.log(
    chalk.bold(
      `Found ${orphans.length} orphan sandbox(es) under ${tmpDir} (total ${formatBytes(
        totalBytes
      )}):`
    )
  );
  console.log();
  for (const o of orphans) {
    console.log(
      `  ${formatAge(o.ageSeconds).padStart(8)}  ${formatBytes(o.sizeBytes).padStart(9)}  ${o.path}`
    );
  }
  if (skippedYoung > 0) {
    console.log(
      chalk.dim(
        `  (${skippedYoung} younger than ${opts.minAgeSeconds ?? 60}s — skipped, likely in-flight)`
      )
    );
  }
  console.log();

  if (!opts.yes) {
    console.log(chalk.yellow("Dry run — pass --yes to actually delete these directories."));
    return 0;
  }

  let removed = 0;
  let failed = 0;
  for (const o of orphans) {
    try {
      await fs.rm(o.path, { recursive: true, force: true });
      removed++;
    } catch (err) {
      console.error(chalk.red(`  ✗ ${o.path}: ${(err as Error).message}`));
      failed++;
    }
  }
  console.log(
    chalk.green(`Removed ${removed} sandbox(es), freed ${formatBytes(totalBytes)}.`) +
      (failed > 0 ? chalk.red(` ${failed} failed.`) : "")
  );
  return failed > 0 ? 1 : 0;
}

async function dirSize(dirPath: string): Promise<number> {
  let total = 0;
  async function walk(cur: string): Promise<void> {
    let entries;
    try {
      entries = await fs.readdir(cur, { withFileTypes: true });
    } catch {
      return;
    }
    for (const e of entries) {
      const full = path.join(cur, e.name);
      if (e.isSymbolicLink()) continue;
      if (e.isDirectory()) {
        await walk(full);
      } else if (e.isFile()) {
        try {
          const st = await fs.stat(full);
          total += st.size;
        } catch {
          // skip
        }
      }
    }
  }
  await walk(dirPath);
  return total;
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

function formatAge(seconds: number): string {
  if (seconds < 60) return `${seconds}s`;
  const m = Math.floor(seconds / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h${m % 60}m`;
  const d = Math.floor(h / 24);
  return `${d}d${h % 24}h`;
}

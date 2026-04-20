import path from "node:path";
import fs from "node:fs/promises";
import { createHash } from "node:crypto";

/**
 * Compute a stable SHA256 of the skill directory: for each file (sorted by
 * relative path), feed `path:<relpath>\n` + contents + `\n` into the hasher.
 */
export async function hashSkillDir(dirPath: string): Promise<string> {
  const files = await listFilesRecursive(dirPath);
  if (files.length === 0) {
    throw new Error(`Skill dir ${dirPath} is empty or unreadable — cannot compute hash.`);
  }
  const hasher = createHash("sha256");
  for (const abs of files) {
    const buf = await fs.readFile(abs);
    const rel = path.relative(dirPath, abs).replaceAll("\\", "/");
    hasher.update(`path:${rel}\n`);
    hasher.update(buf);
    hasher.update("\n");
  }
  return hasher.digest("hex");
}

async function listFilesRecursive(dirPath: string): Promise<string[]> {
  const out: string[] = [];
  async function walk(cur: string): Promise<void> {
    const entries = await fs.readdir(cur, { withFileTypes: true });
    entries.sort((a, b) => a.name.localeCompare(b.name));
    for (const entry of entries) {
      const full = path.join(cur, entry.name);
      if (entry.isDirectory()) {
        await walk(full);
      } else if (entry.isFile()) {
        out.push(full);
      }
    }
  }
  try {
    const st = await fs.stat(dirPath);
    if (!st.isDirectory()) return [];
  } catch {
    return [];
  }
  await walk(dirPath);
  out.sort((a, b) => a.localeCompare(b));
  return out;
}

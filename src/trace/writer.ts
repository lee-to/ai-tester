import fs from "node:fs/promises";
import path from "node:path";
import { getRunsDir } from "../fs/paths.js";
import type { TraceRecord } from "../types.js";

export async function writeTrace(record: TraceRecord): Promise<string> {
  // Sanitize the skill "name" for use as a directory. Inline-prompt scenarios
  // use `inline:<scenario>` which contains `:` (not filesystem-safe on Windows).
  const safeName = record.skill.name.replace(/[^a-zA-Z0-9_.-]+/g, "_");
  const dir = path.join(getRunsDir(), safeName);
  await fs.mkdir(dir, { recursive: true });
  const safeRunId = record.runId.replace(/[^a-zA-Z0-9_.-]+/g, "_");
  const filename = `${safeRunId}.json`;
  const filePath = path.join(dir, filename);
  await fs.writeFile(filePath, JSON.stringify(record, null, 2) + "\n", "utf-8");
  return filePath;
}

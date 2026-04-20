import path from "node:path";
import fs from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { loadProjectConfig, type ProjectConfig } from "../config/project-config.js";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

/** Path to the ai-tester package itself, regardless of where it's installed. */
export function getPackageRoot(): string {
  return path.resolve(__dirname, "..", "..");
}

let cachedConfig: ProjectConfig | null = null;

/** Load (and memoize) the project config — `.ai-tester.yaml` walking up from cwd. */
export async function getProjectConfig(): Promise<ProjectConfig> {
  if (cachedConfig) return cachedConfig;
  cachedConfig = await loadProjectConfig();
  return cachedConfig;
}

export async function getSkillsDir(): Promise<string> {
  return (await getProjectConfig()).skillsDir;
}

export async function getSkillDir(name: string): Promise<string> {
  return path.join(await getSkillsDir(), name);
}

export async function getSkillMdPath(name: string): Promise<string> {
  return path.join(await getSkillDir(name), "SKILL.md");
}

export async function getScenariosDir(skillName: string): Promise<string> {
  return path.join(await getSkillDir(skillName), "tests");
}

export function getRunsDir(): string {
  return path.join(getPackageRoot(), "runs");
}

export function getCacheDir(): string {
  return path.join(getPackageRoot(), "cache");
}

export async function dirExists(p: string): Promise<boolean> {
  try {
    const st = await fs.stat(p);
    return st.isDirectory();
  } catch {
    return false;
  }
}

export async function fileExists(p: string): Promise<boolean> {
  try {
    const st = await fs.stat(p);
    return st.isFile();
  } catch {
    return false;
  }
}

export async function listSkillNames(): Promise<string[]> {
  const skillsDir = await getSkillsDir();
  if (!(await dirExists(skillsDir))) return [];
  const entries = await fs.readdir(skillsDir, { withFileTypes: true });
  const names: string[] = [];
  for (const entry of entries) {
    if (!entry.isDirectory()) continue;
    const skillMd = path.join(skillsDir, entry.name, "SKILL.md");
    try {
      await fs.access(skillMd);
      names.push(entry.name);
    } catch {
      // not a skill directory — skip
    }
  }
  return names.sort();
}

/** For tests / cache reset. */
export function resetConfigCache(): void {
  cachedConfig = null;
}

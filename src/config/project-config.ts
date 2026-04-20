import fs from "node:fs/promises";
import path from "node:path";
import YAML from "yaml";
import { z } from "zod";

const ConfigSchema = z.object({
  skills_dir: z.string().optional(),
  defaults: z
    .object({
      model: z.string().optional(),
      permission_mode: z
        .enum(["acceptEdits", "bypassPermissions", "plan", "default"])
        .optional(),
    })
    .optional(),
});

export type RawProjectConfig = z.infer<typeof ConfigSchema>;

export interface ProjectConfig {
  /** Directory that contains the configuration file, if one was found. */
  rootDir: string;
  /** Path to `.ai-tester.yaml` if it was loaded. */
  configPath: string | null;
  /** Resolved absolute path to the skills directory (may or may not exist). */
  skillsDir: string;
  defaults: {
    model?: string;
    permissionMode?: "acceptEdits" | "bypassPermissions" | "plan" | "default";
  };
}

const CONFIG_FILENAME = ".ai-tester.yaml";

/**
 * Load `.ai-tester.yaml` by walking up from `startDir` (default: cwd) to the
 * filesystem root. If not found, returns a config rooted at cwd with
 * `skills_dir = ./skills`.
 */
export async function loadProjectConfig(startDir: string = process.cwd()): Promise<ProjectConfig> {
  let current = path.resolve(startDir);
  let configPath: string | null = null;

  while (true) {
    const candidate = path.join(current, CONFIG_FILENAME);
    try {
      await fs.access(candidate);
      configPath = candidate;
      break;
    } catch {
      // not here — go up
    }
    const parent = path.dirname(current);
    if (parent === current) break;
    current = parent;
  }

  if (!configPath) {
    const root = path.resolve(startDir);
    return {
      rootDir: root,
      configPath: null,
      skillsDir: path.join(root, "skills"),
      defaults: {},
    };
  }

  const raw = await fs.readFile(configPath, "utf-8");
  let parsed: RawProjectConfig;
  try {
    parsed = ConfigSchema.parse(YAML.parse(raw));
  } catch (err) {
    throw new Error(
      `Invalid ${CONFIG_FILENAME} at ${configPath}: ${(err as Error).message}`
    );
  }

  const configDir = path.dirname(configPath);
  const skillsDir = parsed.skills_dir
    ? path.resolve(configDir, parsed.skills_dir)
    : path.join(configDir, "skills");

  return {
    rootDir: configDir,
    configPath,
    skillsDir,
    defaults: {
      model: parsed.defaults?.model,
      permissionMode: parsed.defaults?.permission_mode,
    },
  };
}

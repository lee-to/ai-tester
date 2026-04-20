import fs from "node:fs/promises";
import path from "node:path";
import YAML from "yaml";
import { ScenarioSchema, type Scenario } from "./schema.js";
import { getScenariosDir, dirExists } from "../fs/paths.js";
import type { LoadedScenario } from "../types.js";

export type { Scenario };

export async function loadScenariosForSkill(skillName: string): Promise<LoadedScenario[]> {
  const dir = await getScenariosDir(skillName);
  if (!(await dirExists(dir))) return [];
  const entries = await fs.readdir(dir, { withFileTypes: true });
  const files = entries
    .filter((e) => e.isFile() && /\.ya?ml$/i.test(e.name) && !e.name.startsWith("_"))
    .map((e) => path.join(dir, e.name))
    .sort();

  const out: LoadedScenario[] = [];
  for (const filePath of files) {
    out.push(await loadScenarioFile(filePath));
  }
  return out;
}

export async function loadScenarioFile(filePath: string): Promise<LoadedScenario> {
  const absPath = path.resolve(filePath);
  const raw = await fs.readFile(absPath, "utf-8");
  let parsed: unknown;
  try {
    parsed = YAML.parse(raw);
  } catch (err) {
    throw new Error(`YAML parse error in ${absPath}: ${(err as Error).message}`);
  }
  const result = ScenarioSchema.safeParse(parsed);
  if (!result.success) {
    const issues = result.error.issues
      .map((i) => `  - ${i.path.join(".") || "<root>"}: ${i.message}`)
      .join("\n");
    throw new Error(`Scenario validation failed for ${absPath}:\n${issues}`);
  }
  await materializeFixtures(result.data, path.dirname(absPath));
  return { scenario: result.data, filePath: absPath };
}

/**
 * Inline `content_from` file contents and resolve `copy_trees[].from` to
 * absolute paths. Mutates `scenario.fixtures` in place.
 */
async function materializeFixtures(
  scenario: Scenario,
  scenarioDir: string
): Promise<void> {
  const fileLists = [
    scenario.fixtures.files_committed,
    scenario.fixtures.files_staged,
    scenario.fixtures.files_unstaged,
  ];
  for (const list of fileLists) {
    for (const file of list) {
      if (file.content_from) {
        const abs = path.resolve(scenarioDir, file.content_from);
        try {
          file.content = await fs.readFile(abs, "utf-8");
        } catch (err) {
          throw new Error(
            `fixture content_from unreadable: ${abs} — ${(err as Error).message}`
          );
        }
      } else if (typeof file.content !== "string") {
        file.content = "";
      }
    }
  }
  for (const tree of scenario.fixtures.copy_trees) {
    const abs = path.resolve(scenarioDir, tree.from);
    try {
      const st = await fs.stat(abs);
      if (!st.isDirectory()) {
        throw new Error(`copy_trees.from must be a directory: ${abs}`);
      }
    } catch (err) {
      throw new Error(
        `copy_trees.from unreadable: ${abs} — ${(err as Error).message}`
      );
    }
    tree.from = abs;
  }
}

/**
 * Resolve the effective system prompt for a scenario:
 *   - `system_prompt`: returned verbatim.
 *   - `system_prompt_file`: path resolved relative to the scenario YAML.
 *   - `skill`: null — caller will load the skill and use its body.
 */
export async function resolveInlineSystemPrompt(
  loaded: LoadedScenario
): Promise<string | null> {
  const { scenario, filePath } = loaded;
  if (typeof scenario.system_prompt === "string") return scenario.system_prompt;
  if (typeof scenario.system_prompt_file === "string") {
    const rel = scenario.system_prompt_file;
    const abs = path.resolve(path.dirname(filePath), rel);
    try {
      return await fs.readFile(abs, "utf-8");
    } catch (err) {
      throw new Error(
        `system_prompt_file not readable: ${abs} — ${(err as Error).message}`
      );
    }
  }
  return null;
}

export async function findScenarioInLoaded(
  loaded: LoadedScenario[],
  scenarioId: string
): Promise<LoadedScenario | null> {
  return loaded.find((l) => l.scenario.scenario === scenarioId) ?? null;
}

export function filterScenarios(
  loaded: LoadedScenario[],
  filter: string | undefined
): LoadedScenario[] {
  if (!filter) return loaded;
  const re = new RegExp(filter);
  return loaded.filter((l) => re.test(l.scenario.scenario));
}

import { getSkillDir, getSkillMdPath, dirExists, fileExists } from "../fs/paths.js";
import type { SkillRecord } from "../types.js";
import { parseSkillMd } from "./parse.js";
import { tokenizeAllowedTools } from "./allowed-tools.js";
import { hashSkillDir } from "./version.js";

export async function loadSkill(name: string): Promise<SkillRecord> {
  const dirPath = await getSkillDir(name);
  if (!(await dirExists(dirPath))) {
    throw new Error(`Skill directory not found: ${dirPath}`);
  }
  const skillMdPath = await getSkillMdPath(name);
  if (!(await fileExists(skillMdPath))) {
    throw new Error(`SKILL.md not found at ${skillMdPath}`);
  }
  const { frontmatter, body, bodyHash } = await parseSkillMd(skillMdPath);
  if (frontmatter.name !== name) {
    throw new Error(
      `Skill name mismatch: directory is "${name}" but frontmatter.name is "${frontmatter.name}".`
    );
  }
  const { raw, parsed } = tokenizeAllowedTools(frontmatter["allowed-tools"]);
  const sourceHash = await hashSkillDir(dirPath);
  return {
    name,
    dirPath,
    skillMdPath,
    frontmatter,
    body,
    bodyHash,
    sourceHash,
    allowedTools: parsed,
    allowedToolsRaw: raw,
  };
}

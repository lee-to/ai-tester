import fs from "node:fs/promises";
import matter from "gray-matter";
import { createHash } from "node:crypto";
import type { SkillFrontmatter } from "../types.js";

export interface ParsedSkillFile {
  frontmatter: SkillFrontmatter;
  body: string;
  bodyHash: string;
}

export async function parseSkillMd(filePath: string): Promise<ParsedSkillFile> {
  const raw = await fs.readFile(filePath, "utf-8");
  const parsed = matter(raw);
  const frontmatter = parsed.data as SkillFrontmatter;
  if (!frontmatter || typeof frontmatter !== "object") {
    throw new Error(`SKILL.md at ${filePath} has no valid YAML frontmatter.`);
  }
  if (!frontmatter.name || !frontmatter.description) {
    throw new Error(
      `SKILL.md at ${filePath} missing required frontmatter (name, description).`
    );
  }
  const body = parsed.content;
  const bodyHash = createHash("sha256").update(body).digest("hex");
  return { frontmatter, body, bodyHash };
}

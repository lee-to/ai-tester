import type { ParsedTool } from "../types.js";

export interface AllowedToolsTokenized {
  raw: string[];
  parsed: ParsedTool[];
}

/**
 * Tokenize an `allowed-tools` frontmatter string into raw whitespace-separated
 * tokens (respecting parens) and a deduplicated, parsed list of tools.
 *
 * Grammar examples:
 *   "Read Bash(git *) AskUserQuestion"
 *   "Bash(mkdir, npx, python) Bash(git *)"
 *   "mcp__handoff__handoff_sync_status Read"
 */
export function tokenizeAllowedTools(
  input: string | undefined | null
): AllowedToolsTokenized {
  if (!input) return { raw: [], parsed: [] };

  const raw: string[] = [];
  let depth = 0;
  let buf = "";
  for (const ch of input) {
    if (ch === "(") {
      depth++;
      buf += ch;
    } else if (ch === ")") {
      depth = Math.max(0, depth - 1);
      buf += ch;
    } else if (/\s/.test(ch) && depth === 0) {
      if (buf.length > 0) {
        raw.push(buf);
        buf = "";
      }
    } else {
      buf += ch;
    }
  }
  if (buf.length > 0) raw.push(buf);

  const merged = new Map<string, Set<string>>();
  for (const token of raw) {
    const open = token.indexOf("(");
    if (open === -1) {
      if (!merged.has(token)) merged.set(token, new Set());
      continue;
    }
    const name = token.slice(0, open);
    const close = token.lastIndexOf(")");
    const rawScopes = close > open ? token.slice(open + 1, close) : token.slice(open + 1);
    const scopes = rawScopes
      .split(",")
      .map((s) => s.trim())
      .filter((s) => s.length > 0);
    if (!merged.has(name)) merged.set(name, new Set());
    const set = merged.get(name)!;
    for (const s of scopes) set.add(s);
  }

  const parsed: ParsedTool[] = [...merged.entries()].map(([name, set]) => ({
    name,
    scopes: [...set],
  }));

  return { raw, parsed };
}

/** Backwards-compat wrapper. */
export function parseAllowedTools(input: string | undefined | null): ParsedTool[] {
  return tokenizeAllowedTools(input).parsed;
}

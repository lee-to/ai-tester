import type { ToolCallRecord, TraceRecord } from "../types.js";
import { compilePattern } from "../util/regex.js";

/** Flattened list of tool calls in trace order. */
export function collectToolCalls(trace: TraceRecord): ToolCallRecord[] {
  const out: ToolCallRecord[] = [];
  for (const turn of trace.turns) {
    for (const tc of turn.toolCalls ?? []) out.push(tc);
  }
  return out;
}

export function argsMatchInput(
  match: Record<string, string> | undefined,
  input: Record<string, unknown>
): boolean {
  if (!match) return true;
  for (const [key, pattern] of Object.entries(match)) {
    const actual = input[key];
    const str = actual === undefined || actual === null ? "" : String(actual);
    try {
      if (!compilePattern(pattern).test(str)) return false;
    } catch {
      return false;
    }
  }
  return true;
}

export function summarizeCall(tc: ToolCallRecord): string {
  const preview = Object.entries(tc.input)
    .slice(0, 2)
    .map(([k, v]) => `${k}=${truncate(JSON.stringify(v), 50)}`)
    .join(", ");
  return preview ? `${tc.name}(${preview})` : tc.name;
}

export function truncate(s: string, n: number): string {
  return s.length > n ? s.slice(0, n - 1) + "…" : s;
}

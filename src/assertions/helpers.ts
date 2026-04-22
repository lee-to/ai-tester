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

export const DEFAULT_CAPTURE_MAX_CHARS = 2000;

/** Stringify an input field value and cap its length for `capture:` output. */
export function captureFields(
  input: Record<string, unknown>,
  fields: string[] | undefined,
  maxChars: number | undefined,
  step?: number
): Array<{
  field: string;
  value: string;
  truncated: boolean;
  originalLength: number;
  step?: number;
}> {
  if (!fields || fields.length === 0) return [];
  const cap = maxChars ?? DEFAULT_CAPTURE_MAX_CHARS;
  const out: Array<{
    field: string;
    value: string;
    truncated: boolean;
    originalLength: number;
    step?: number;
  }> = [];
  for (const field of fields) {
    const raw = input[field];
    const str = raw === undefined || raw === null ? "" : typeof raw === "string" ? raw : JSON.stringify(raw, null, 2);
    const truncated = str.length > cap;
    const value = truncated ? str.slice(0, cap) : str;
    const entry: {
      field: string;
      value: string;
      truncated: boolean;
      originalLength: number;
      step?: number;
    } = { field, value, truncated, originalLength: str.length };
    if (step !== undefined) entry.step = step;
    out.push(entry);
  }
  return out;
}

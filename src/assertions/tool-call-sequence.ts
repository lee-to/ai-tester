import type { AssertionResult, TraceRecord } from "../types.js";
import { collectToolCalls, argsMatchInput, summarizeCall } from "./helpers.js";

export interface ToolCallSequenceSpec {
  id: string;
  type: "tool_call_sequence";
  sequence: Array<{ tool: string; args_match?: Record<string, string> }>;
  weight?: number;
}

export function evaluateToolCallSequence(
  spec: ToolCallSequenceSpec,
  trace: TraceRecord
): AssertionResult {
  const calls = collectToolCalls(trace);
  let cursor = 0;
  for (let step = 0; step < spec.sequence.length; step++) {
    const want = spec.sequence[step]!;
    let found = -1;
    for (let i = cursor; i < calls.length; i++) {
      const tc = calls[i]!;
      if (tc.name !== want.tool) continue;
      if (!argsMatchInput(want.args_match, tc.input)) continue;
      found = i;
      break;
    }
    if (found === -1) {
      const matched = calls.slice(0, cursor).map(summarizeCall).join(" → ");
      return {
        id: spec.id,
        type: "tool_call_sequence",
        pass: false,
        weight: spec.weight ?? 1,
        detail:
          `step ${step + 1}/${spec.sequence.length}: expected ${want.tool}` +
          (want.args_match ? ` ${describeArgsMatch(want.args_match)}` : "") +
          ` after position ${cursor}. Matched so far: [${matched || "nothing"}]`,
      };
    }
    cursor = found + 1;
  }
  return {
    id: spec.id,
    type: "tool_call_sequence",
    pass: true,
    weight: spec.weight ?? 1,
    detail: `matched all ${spec.sequence.length} step(s)`,
  };
}

function describeArgsMatch(match: Record<string, string>): string {
  const fields = Object.entries(match)
    .map(([k, v]) => `${k}~/${v}/`)
    .join(", ");
  return `{${fields}}`;
}

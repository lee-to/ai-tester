import type { AssertionResult, TraceRecord } from "../types.js";
import { collectToolCalls, argsMatchInput, summarizeCall } from "./helpers.js";

export interface NoToolCalledSpec {
  id: string;
  type: "no_tool_called";
  tool: string;
  args_match?: Record<string, string>;
  weight?: number;
}

export function evaluateNoToolCalled(
  spec: NoToolCalledSpec,
  trace: TraceRecord
): AssertionResult {
  const calls = collectToolCalls(trace);
  for (const tc of calls) {
    if (tc.name !== spec.tool) continue;
    if (!argsMatchInput(spec.args_match, tc.input)) continue;
    return {
      id: spec.id,
      type: "no_tool_called",
      pass: false,
      weight: spec.weight ?? 1,
      detail: `unexpected call: ${summarizeCall(tc)}`,
    };
  }
  return {
    id: spec.id,
    type: "no_tool_called",
    pass: true,
    weight: spec.weight ?? 1,
    detail: `no matching ${spec.tool} calls found`,
  };
}

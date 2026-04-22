import type { AssertionResult, TraceRecord } from "../types.js";
import {
  collectToolCalls,
  argsMatchInput,
  summarizeCall,
  captureFields,
} from "./helpers.js";

export interface ToolCalledSpec {
  id: string;
  type: "tool_called";
  tool: string;
  args_match?: Record<string, string>;
  call_index?: number;
  weight?: number;
  capture?: string[];
  capture_max_chars?: number;
}

export function evaluateToolCalled(
  spec: ToolCalledSpec,
  trace: TraceRecord
): AssertionResult {
  const calls = collectToolCalls(trace);
  let callIndexForTool = 0;
  for (const tc of calls) {
    if (tc.name !== spec.tool) continue;
    const currentIndex = callIndexForTool++;
    if (spec.call_index !== undefined && spec.call_index !== currentIndex) continue;
    if (!argsMatchInput(spec.args_match, tc.input)) continue;
    const captures = captureFields(tc.input, spec.capture, spec.capture_max_chars);
    return {
      id: spec.id,
      type: "tool_called",
      pass: true,
      weight: spec.weight ?? 1,
      detail: `matched ${summarizeCall(tc)} at position ${currentIndex}`,
      ...(captures.length > 0 ? { captures } : {}),
    };
  }
  const matchDesc = describeArgsMatch(spec);
  return {
    id: spec.id,
    type: "tool_called",
    pass: false,
    weight: spec.weight ?? 1,
    detail: `no ${spec.tool} call${matchDesc} found`,
  };
}

function describeArgsMatch(spec: ToolCalledSpec): string {
  if (!spec.args_match && spec.call_index === undefined) return "";
  const parts: string[] = [];
  if (spec.args_match) {
    const fields = Object.entries(spec.args_match)
      .map(([k, v]) => `${k}~/${v}/`)
      .join(", ");
    parts.push(`matching {${fields}}`);
  }
  if (spec.call_index !== undefined) parts.push(`at index ${spec.call_index}`);
  return " " + parts.join(" ");
}

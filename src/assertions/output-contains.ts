import type { AssertionResult, TraceRecord } from "../types.js";
import { truncate } from "./helpers.js";
import { compilePattern } from "../util/regex.js";

export interface OutputContainsSpec {
  id: string;
  type: "output_contains";
  pattern: string;
  weight?: number;
}

export function evaluateOutputContains(
  spec: OutputContainsSpec,
  trace: TraceRecord
): AssertionResult {
  let re: RegExp;
  try {
    re = compilePattern(spec.pattern);
  } catch (err) {
    return {
      id: spec.id,
      type: "output_contains",
      pass: false,
      weight: spec.weight ?? 1,
      detail: `invalid regex "${spec.pattern}": ${(err as Error).message}`,
    };
  }
  const pass = re.test(trace.finalOutput);
  return {
    id: spec.id,
    type: "output_contains",
    pass,
    weight: spec.weight ?? 1,
    detail: pass
      ? `pattern /${spec.pattern}/ matched`
      : `pattern /${spec.pattern}/ did not match final output: "${truncate(trace.finalOutput, 120)}"`,
  };
}

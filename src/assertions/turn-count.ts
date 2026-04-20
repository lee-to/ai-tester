import type { AssertionResult, TraceRecord } from "../types.js";

export interface TurnCountAtMostSpec {
  id: string;
  type: "turn_count_at_most";
  max: number;
  weight?: number;
}

export function evaluateTurnCountAtMost(
  spec: TurnCountAtMostSpec,
  trace: TraceRecord
): AssertionResult {
  const used = trace.runner.turnsUsed;
  const pass = used <= spec.max;
  return {
    id: spec.id,
    type: "turn_count_at_most",
    pass,
    weight: spec.weight ?? 1,
    detail: pass
      ? `${used} turn(s) ≤ ${spec.max}`
      : `${used} turn(s) exceeds budget of ${spec.max}`,
  };
}

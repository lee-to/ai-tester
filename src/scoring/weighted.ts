import type { AssertionResult } from "../types.js";

/**
 * Weighted pass rate across assertion results. Each assertion contributes
 * its weight multiplied by its pass (1) / fail (0) value (or its `score`
 * field if set for future LLM-judge assertions). Returns a number in [0, 1].
 */
export function computeWeightedScore(results: AssertionResult[]): number {
  if (results.length === 0) return 1;
  let totalWeight = 0;
  let totalAchieved = 0;
  for (const r of results) {
    const weight = r.weight ?? 1;
    const achieved = typeof r.score === "number" ? r.score : r.pass ? 1 : 0;
    totalWeight += weight;
    totalAchieved += weight * achieved;
  }
  return totalWeight === 0 ? 1 : totalAchieved / totalWeight;
}

import type { Assertion } from "../scenario/schema.js";
import type { AssertionResult, TraceRecord } from "../types.js";
import { evaluateToolCalled } from "./tool-called.js";
import { evaluateToolCallSequence } from "./tool-call-sequence.js";
import { evaluateNoToolCalled } from "./no-tool-called.js";
import { evaluateOutputContains } from "./output-contains.js";
import { evaluateTurnCountAtMost } from "./turn-count.js";
import { evaluateNoPathEscape } from "./no-path-escape.js";

export function evaluateAssertions(
  assertions: Assertion[],
  trace: TraceRecord
): AssertionResult[] {
  const results: AssertionResult[] = [];
  for (const spec of assertions) {
    switch (spec.type) {
      case "tool_called":
        results.push(evaluateToolCalled(spec, trace));
        break;
      case "tool_call_sequence":
        results.push(evaluateToolCallSequence(spec, trace));
        break;
      case "no_tool_called":
        results.push(evaluateNoToolCalled(spec, trace));
        break;
      case "output_contains":
        results.push(evaluateOutputContains(spec, trace));
        break;
      case "turn_count_at_most":
        results.push(evaluateTurnCountAtMost(spec, trace));
        break;
      case "no_path_escape":
        results.push(evaluateNoPathEscape(spec, trace));
        break;
    }
  }
  results.push(evaluateNoUnansweredQuestions(trace));
  const budget = evaluateTokenBudget(trace);
  if (budget) results.push(budget);
  return results;
}

/**
 * Implicit assertion: every AskUserQuestion that fired was matched by a
 * `user_responses` entry. Always on.
 */
function evaluateNoUnansweredQuestions(trace: TraceRecord): AssertionResult {
  const unanswered = trace.toolCallSummary.unansweredQuestions;
  const pass = unanswered === 0;
  return {
    id: "no_unanswered_questions",
    type: "no_unanswered_questions",
    pass,
    weight: 1,
    detail: pass
      ? "all AskUserQuestion calls had matching user_responses entries"
      : `${unanswered} AskUserQuestion call(s) had no matching user_responses — add entries or tighten match_question regex`,
  };
}

/**
 * Implicit assertion: if the scenario or its skill declares `token_budget` /
 * `token-budget`, the run must not exceed it. Scenario wins over skill.
 * Only emitted when one of them sets a budget.
 */
function evaluateTokenBudget(trace: TraceRecord): AssertionResult | null {
  const fromScenario = trace.scenario.tokenBudget;
  const fromSkill = trace.skill.tokenBudget;
  const budget = fromScenario ?? fromSkill;
  if (budget == null) return null;
  const source = fromScenario != null ? "scenario" : "skill";
  const { inputTokens, outputTokens, cacheCreationTokens, cacheReadTokens } = trace.cost;
  const total = inputTokens + outputTokens + cacheCreationTokens + cacheReadTokens;
  const pass = total <= budget;
  return {
    id: "token_budget",
    type: "token_budget",
    pass,
    weight: 1,
    detail: pass
      ? `${total.toLocaleString("en-US")} tokens ≤ ${source} budget of ${budget.toLocaleString("en-US")}`
      : `${total.toLocaleString("en-US")} tokens exceeds ${source} \`token_budget\` of ${budget.toLocaleString("en-US")} (input=${inputTokens}, output=${outputTokens}, cache-creation=${cacheCreationTokens}, cache-read=${cacheReadTokens})`,
  };
}

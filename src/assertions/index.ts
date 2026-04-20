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

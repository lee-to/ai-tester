// Smoke test for the assertion evaluators. Builds synthetic traces (matching
// the 1.1.0 TraceRecord schema) and verifies pass/fail outcomes. No SDK, no
// sandbox — pure logic check.

import { evaluateAssertions } from "../dist/assertions/index.js";
import { computeWeightedScore } from "../dist/scoring/weighted.js";

const baseRecord = {
  schemaVersion: "1.1.0",
  runId: "smoke",
  skill: {
    name: "aif-commit",
    path: "skills/aif-commit/SKILL.md",
    version: null,
    sourceHash: "0".repeat(64),
    sourceHashShort: "00000000",
    bodyHash: "0".repeat(64),
    allowedToolsParsed: [
      { name: "Bash", scopes: ["git *"] },
      { name: "Read", scopes: [] },
    ],
    allowedToolsRaw: ["Bash(git *)", "Read"],
  },
  scenario: { name: "synth", path: "", argument: null },
  runner: {
    model: "claude-sonnet-4-6",
    permissionMode: "bypassPermissions",
    startedAt: "", finishedAt: "", durationMs: 0,
    maxTurns: 12, turnsUsed: 5, sessionId: null, sandboxPath: null,
  },
  turns: [
    {
      index: 0, role: "assistant", textDeltas: [],
      toolCalls: [
        { id: "1", name: "Bash", input: { command: "git status" }, resultContent: "clean", resultIsError: false, answered: null },
      ],
    },
    {
      index: 1, role: "assistant", textDeltas: [],
      toolCalls: [
        { id: "2", name: "Bash", input: { command: "git diff --cached" }, resultContent: "+feat", resultIsError: false, answered: null },
      ],
    },
    {
      index: 2, role: "assistant", textDeltas: [],
      toolCalls: [
        { id: "3", name: "AskUserQuestion", input: { questions: [{ question: "Proposed commit message" }] }, resultContent: "Commit as is", resultIsError: false, answered: { matchedEntryIndex: 0, chosenLabel: "Commit as is" } },
      ],
    },
    {
      index: 3, role: "assistant", textDeltas: [],
      toolCalls: [
        { id: "4", name: "Bash", input: { command: "git commit -m feat" }, resultContent: "committed", resultIsError: false, answered: null },
      ],
    },
  ],
  finalOutput: "Commit created: feat(auth): add password reset",
  toolCallSummary: { total: 4, byTool: { Bash: 3, AskUserQuestion: 1 }, unansweredQuestions: 0 },
  assertions: [],
  scoring: { allPassed: true, overallPass: true },
  cost: { inputTokens: 0, outputTokens: 0, cacheCreationTokens: 0, cacheReadTokens: 0, usdEstimate: 0, source: "unknown" },
  errors: [],
};

const assertions = [
  { id: "calls-git-status", type: "tool_called", tool: "Bash", args_match: { command: "^git status" }, weight: 1 },
  { id: "diff-before-commit", type: "tool_call_sequence", sequence: [
      { tool: "Bash", args_match: { command: "^git diff --cached" } },
      { tool: "AskUserQuestion" },
      { tool: "Bash", args_match: { command: "^git commit" } },
    ], weight: 2 },
  { id: "no-write-tool", type: "no_tool_called", tool: "Write", weight: 1 },
  { id: "no-unscoped-bash", type: "no_tool_called", tool: "Bash", args_match: { command: "^(?!git )" }, weight: 1 },
  { id: "mentions-feat", type: "output_contains", pattern: "\\bfeat\\b", weight: 1 },
  { id: "efficient", type: "turn_count_at_most", max: 8, weight: 1 },
];

const pass1 = evaluateAssertions(assertions, baseRecord);
console.log("=== happy path ===");
for (const r of pass1) console.log(`  ${r.pass ? "OK" : "FAIL"} ${r.id}: ${r.detail}`);
const scoreHappy = computeWeightedScore(pass1);
console.log(`  score: ${scoreHappy.toFixed(2)}`);
if (!pass1.every((r) => r.pass)) {
  console.error("!! happy path should all pass");
  process.exit(1);
}

// Negative: add an unscoped Bash call, a Write call, unanswered question, and
// bump turnsUsed over budget.
const brokenRecord = structuredClone(baseRecord);
brokenRecord.turns.push({
  index: 4, role: "assistant", textDeltas: [],
  toolCalls: [
    { id: "5", name: "Bash", input: { command: "ls -la" }, resultContent: null, resultIsError: false, answered: null },
    { id: "6", name: "Write", input: { file_path: "/tmp/x", content: "" }, resultContent: null, resultIsError: false, answered: null },
  ],
});
brokenRecord.finalOutput = "Commit created: added stuff"; // no "feat" word
brokenRecord.runner.turnsUsed = 10;
brokenRecord.toolCallSummary.unansweredQuestions = 1;

const pass2 = evaluateAssertions(assertions, brokenRecord);
console.log("\n=== broken path ===");
for (const r of pass2) console.log(`  ${r.pass ? "OK" : "FAIL"} ${r.id}: ${r.detail}`);

const expectedFails = new Set([
  "no-write-tool",
  "no-unscoped-bash",
  "mentions-feat",
  "efficient",
  "no_unanswered_questions",
]);
const actualFails = new Set(pass2.filter((r) => !r.pass).map((r) => r.id));
const missing = [...expectedFails].filter((id) => !actualFails.has(id));
const extra = [...actualFails].filter((id) => !expectedFails.has(id));

if (missing.length || extra.length) {
  console.error(`\n!! mismatch. missing fails: ${missing.join(", ") || "(none)"}, unexpected fails: ${extra.join(", ") || "(none)"}`);
  process.exit(1);
}

console.log("\nSMOKE OK");

import { test } from "node:test";
import assert from "node:assert/strict";
import { ScenarioSchema } from "../../dist/scenario/schema.js";

test("ScenarioSchema: minimal skill-backed scenario parses with defaults", () => {
  const parsed = ScenarioSchema.parse({
    scenario: "basic",
    skill: "aif-commit",
  });
  assert.equal(parsed.scenario, "basic");
  assert.equal(parsed.skill, "aif-commit");
  assert.equal(parsed.runner.runtime, "claude");
  assert.equal(parsed.runner.permission_mode, "bypassPermissions");
  assert.deepEqual(parsed.assertions, []);
  assert.deepEqual(parsed.user_responses, []);
  assert.equal(parsed.fixtures.git_init, false);
});

test("ScenarioSchema: rejects scenario with zero prompt sources", () => {
  assert.throws(() =>
    ScenarioSchema.parse({
      scenario: "noop",
    })
  );
});

test("ScenarioSchema: rejects scenario with multiple prompt sources", () => {
  assert.throws(() =>
    ScenarioSchema.parse({
      scenario: "conflict",
      skill: "aif-commit",
      system_prompt: "You are a helper.",
    })
  );
  assert.throws(() =>
    ScenarioSchema.parse({
      scenario: "conflict2",
      system_prompt: "Hi",
      system_prompt_file: "./p.md",
    })
  );
});

test("ScenarioSchema: inline system_prompt is accepted alone", () => {
  const parsed = ScenarioSchema.parse({
    scenario: "inline",
    system_prompt: "You are a coding assistant.",
  });
  assert.equal(parsed.system_prompt, "You are a coding assistant.");
  assert.equal(parsed.skill, undefined);
});

test("ScenarioSchema: system_prompt_file alone is accepted", () => {
  const parsed = ScenarioSchema.parse({
    scenario: "ext",
    system_prompt_file: "./prompts/reviewer.md",
  });
  assert.equal(parsed.system_prompt_file, "./prompts/reviewer.md");
});

test("ScenarioSchema: tool_called assertion round-trips", () => {
  const parsed = ScenarioSchema.parse({
    scenario: "tc",
    skill: "aif-commit",
    assertions: [
      {
        id: "calls-git-status",
        type: "tool_called",
        tool: "Bash",
        args_match: { command: "^git status" },
      },
    ],
  });
  assert.equal(parsed.assertions.length, 1);
  assert.equal(parsed.assertions[0].type, "tool_called");
  assert.equal(parsed.assertions[0].weight, 1);
});

test("ScenarioSchema: rejects unknown assertion type", () => {
  assert.throws(() =>
    ScenarioSchema.parse({
      scenario: "bad",
      skill: "aif-commit",
      assertions: [{ id: "x", type: "hallucinated_type" }],
    })
  );
});

test("ScenarioSchema: rejects empty tool_call_sequence", () => {
  assert.throws(() =>
    ScenarioSchema.parse({
      scenario: "bad-seq",
      skill: "aif-commit",
      assertions: [{ id: "s", type: "tool_call_sequence", sequence: [] }],
    })
  );
});

test("ScenarioSchema: fixtures defaults applied when omitted", () => {
  const parsed = ScenarioSchema.parse({
    scenario: "fx",
    skill: "aif-commit",
  });
  assert.deepEqual(parsed.fixtures.copy_trees, []);
  assert.deepEqual(parsed.fixtures.files_committed, []);
  assert.deepEqual(parsed.fixtures.files_staged, []);
  assert.deepEqual(parsed.fixtures.files_unstaged, []);
  assert.deepEqual(parsed.fixtures.setup_commands, []);
  assert.deepEqual(parsed.fixtures.env, {});
});

test("ScenarioSchema: fixture file with both content and content_from is rejected", () => {
  assert.throws(() =>
    ScenarioSchema.parse({
      scenario: "dup",
      skill: "aif-commit",
      fixtures: {
        files_committed: [
          { path: "a.txt", content: "hi", content_from: "./a.txt" },
        ],
      },
    })
  );
});

test("ScenarioSchema: runner.permission_mode enum is enforced", () => {
  assert.throws(() =>
    ScenarioSchema.parse({
      scenario: "pm",
      skill: "aif-commit",
      runner: { permission_mode: "yoloMode" },
    })
  );
});

test("ScenarioSchema: custom max_turns preserved", () => {
  const parsed = ScenarioSchema.parse({
    scenario: "turns",
    skill: "aif-commit",
    max_turns: 12,
  });
  assert.equal(parsed.max_turns, 12);
});

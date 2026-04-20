import { z } from "zod";
import { DEFAULT_MODEL } from "../config.js";

const ArgsMatch = z.record(z.string(), z.string()).optional();

const FixtureFile = z
  .object({
    path: z.string().min(1),
    /** Inline content. Mutually exclusive with `content_from`. */
    content: z.string().optional(),
    /**
     * Load content from a file on disk, path resolved relative to the scenario
     * YAML. Mutually exclusive with `content`. The loader reads the file and
     * populates `content` before fixtures are applied.
     */
    content_from: z.string().min(1).optional(),
  })
  .refine((f) => !(typeof f.content === "string" && f.content_from), {
    message: "fixture file must declare `content` or `content_from`, not both",
  });

const CopyTree = z.object({
  /**
   * Source directory, resolved relative to the scenario YAML. Its CONTENTS are
   * copied (not the directory itself). Symlinks follow Node's `fs.cp` defaults.
   */
  from: z.string().min(1),
  /** Destination relative to the sandbox root. Defaults to the root. */
  to: z.string().default("."),
});

const Fixtures = z.object({
  git_init: z.boolean().default(false),
  git_branch: z.string().optional(),
  /**
   * Directory trees copied into the sandbox BEFORE file-level fixtures.
   * When `git_init` is true the trees are part of the baseline commit (unless
   * overlaid by `files_staged` / `files_unstaged`).
   */
  copy_trees: z.array(CopyTree).default([]),
  /** Files committed as the initial baseline (created AND committed). */
  files_committed: z.array(FixtureFile).default([]),
  /** Files created then staged via `git add` (NOT committed). */
  files_staged: z.array(FixtureFile).default([]),
  /** Files created and left as uncommitted untracked/modified state. */
  files_unstaged: z.array(FixtureFile).default([]),
  /** Arbitrary shell commands to run in the sandbox after file setup. */
  setup_commands: z.array(z.string()).default([]),
  env: z.record(z.string(), z.string()).default({}),
});

const UserResponse = z.object({
  match_question: z.string().min(1),
  choose: z.string().min(1),
});

const Runner = z.object({
  /** Runtime adapter id — registered in src/runtimes/index.ts. */
  runtime: z.string().default("claude"),
  model: z.string().default(DEFAULT_MODEL),
  permission_mode: z
    .enum(["acceptEdits", "bypassPermissions", "plan", "default"])
    .default("bypassPermissions"),
  allowed_tools_override: z.array(z.string()).optional(),
});

const AssertionBase = {
  id: z.string().min(1),
  weight: z.number().positive().default(1),
};

const AssertionToolCalled = z.object({
  ...AssertionBase,
  type: z.literal("tool_called"),
  tool: z.string().min(1),
  args_match: ArgsMatch,
  call_index: z.number().int().nonnegative().optional(),
});

const AssertionToolSequence = z.object({
  ...AssertionBase,
  type: z.literal("tool_call_sequence"),
  sequence: z
    .array(
      z.object({
        tool: z.string().min(1),
        args_match: ArgsMatch,
      })
    )
    .min(1),
});

const AssertionNoToolCalled = z.object({
  ...AssertionBase,
  type: z.literal("no_tool_called"),
  tool: z.string().min(1),
  args_match: ArgsMatch,
});

const AssertionOutputContains = z.object({
  ...AssertionBase,
  type: z.literal("output_contains"),
  pattern: z.string().min(1),
});

const AssertionTurnCount = z.object({
  ...AssertionBase,
  type: z.literal("turn_count_at_most"),
  max: z.number().int().positive(),
});

const AssertionNoPathEscape = z.object({
  ...AssertionBase,
  type: z.literal("no_path_escape"),
  tools: z.array(z.string().min(1)).optional(),
  allow_outside: z.array(z.string().min(1)).optional(),
});

export const AssertionSchema = z.discriminatedUnion("type", [
  AssertionToolCalled,
  AssertionToolSequence,
  AssertionNoToolCalled,
  AssertionOutputContains,
  AssertionTurnCount,
  AssertionNoPathEscape,
]);

export const ScenarioSchema = z
  .object({
    scenario: z.string().min(1),
    description: z.string().optional(),
    /**
     * Skill directory name to load from the configured skills_dir. Mutually
     * exclusive with `system_prompt` and `system_prompt_file`.
     */
    skill: z.string().min(1).optional(),
    /** Inline raw system prompt — for testing bare prompts without a skill. */
    system_prompt: z.string().min(1).optional(),
    /** Path to a system prompt file, relative to the scenario YAML. */
    system_prompt_file: z.string().min(1).optional(),
    argument: z.string().optional(),
    /**
     * Explicit hard cap on turns. When present, exceeding it fails the scenario.
     * When absent, the runner uses an internal safety cap and hitting it emits
     * a warning but does not fail the scenario.
     */
    max_turns: z.number().int().positive().optional(),
    runner: Runner.prefault({}),
    fixtures: Fixtures.prefault({}),
    user_responses: z.array(UserResponse).default([]),
    assertions: z.array(AssertionSchema).default([]),
  })
  .refine(
    (data) => {
      const count =
        (data.skill ? 1 : 0) +
        (data.system_prompt ? 1 : 0) +
        (data.system_prompt_file ? 1 : 0);
      return count === 1;
    },
    {
      message:
        "scenario must declare exactly one of: `skill`, `system_prompt`, or `system_prompt_file`",
      path: ["skill"],
    }
  );

export type Scenario = z.infer<typeof ScenarioSchema>;
export type Assertion = z.infer<typeof AssertionSchema>;
export type UserResponseEntry = z.infer<typeof UserResponse>;
export type FixtureSpec = z.infer<typeof Fixtures>;

import type { Scenario } from "./scenario/schema.js";

export type { Scenario } from "./scenario/schema.js";

export interface ParsedTool {
  name: string;
  scopes: string[];
}

export interface SkillFrontmatter {
  name: string;
  description: string;
  "argument-hint"?: string;
  "allowed-tools"?: string;
  "disable-model-invocation"?: boolean;
  version?: string;
  /** Max total tokens (input+output+cache) the skill is allowed to consume per run. */
  "token-budget"?: number;
}

export interface SkillRecord {
  name: string;
  dirPath: string;
  skillMdPath: string;
  frontmatter: SkillFrontmatter;
  body: string;
  bodyHash: string;
  sourceHash: string;
  allowedTools: ParsedTool[];
  allowedToolsRaw: string[];
}

export interface LoadedScenario {
  scenario: Scenario;
  filePath: string;
}

export interface ToolCallRecord {
  id: string;
  name: string;
  input: Record<string, unknown>;
  resultContent: string | null;
  resultIsError: boolean;
  answered: {
    matchedEntryIndex: number;
    chosenLabel: string;
  } | null;
}

export interface Turn {
  index: number;
  role: "assistant" | "user" | "system";
  textDeltas?: string[];
  toolCalls?: ToolCallRecord[];
  usage?: {
    inputTokens: number;
    cacheCreationInputTokens: number;
    cacheReadInputTokens: number;
    outputTokens: number;
  };
}

export interface AssertionResult {
  id: string;
  type: string;
  pass: boolean;
  detail: string;
  weight: number;
  score?: number;
  minScore?: number;
  rationale?: string;
}

export interface TraceRecord {
  schemaVersion: "1.1.0";
  runId: string;
  skill: {
    name: string;
    path: string;
    version: string | null;
    sourceHash: string;
    sourceHashShort: string;
    bodyHash: string;
    allowedToolsParsed: ParsedTool[];
    allowedToolsRaw: string[];
    tokenBudget: number | null;
  };
  scenario: {
    name: string;
    path: string;
    argument: string | null;
    tokenBudget: number | null;
  };
  runner: {
    model: string;
    permissionMode: string;
    startedAt: string;
    finishedAt: string;
    durationMs: number;
    maxTurns: number;
    maxTurnsUserSet: boolean;
    turnsUsed: number;
    hitMaxTurns: boolean;
    sessionId: string | null;
    sandboxPath: string | null;
  };
  turns: Turn[];
  finalOutput: string;
  toolCallSummary: {
    total: number;
    byTool: Record<string, number>;
    unansweredQuestions: number;
  };
  assertions: AssertionResult[];
  scoring: {
    allPassed: boolean;
    overallPass: boolean;
    weightedScore?: number;
    passThreshold?: number;
  };
  cost: {
    inputTokens: number;
    outputTokens: number;
    cacheCreationTokens: number;
    cacheReadTokens: number;
    usdEstimate: number;
    source: "sdk" | "unknown";
  };
  errors: Array<{ kind: string; message: string; detail?: unknown }>;
}

export interface RunOptions {
  skill?: string;
  scenario?: string;
  file?: string;
  model?: string;
  runtime?: string;
  filter?: string;
  dryRun?: boolean;
  keepSandbox?: boolean;
  quiet?: boolean;
  idleWarnSeconds?: number;
}

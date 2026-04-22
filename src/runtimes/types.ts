import type { Turn, SkillRecord } from "../types.js";
import type { Scenario, UserResponseEntry } from "../scenario/schema.js";

/** Common shape for live-progress events emitted by any runtime. */
export type ProgressEvent =
  | { kind: "system_init"; sessionId: string | null; elapsedMs: number }
  | { kind: "assistant_text"; text: string; elapsedMs: number }
  | { kind: "tool_use"; tool: string; input: Record<string, unknown>; elapsedMs: number }
  | {
      kind: "tool_result";
      tool: string;
      toolUseId: string;
      content: string;
      isError: boolean;
      elapsedMs: number;
    }
  | {
      kind: "question_answered";
      tool: string;
      chosen: string;
      questionPreview: string;
      elapsedMs: number;
    }
  | {
      kind: "question_unanswered";
      tool: string;
      questionPreview: string;
      elapsedMs: number;
    }
  | { kind: "result"; subtype: string | null; usdEstimate: number; elapsedMs: number }
  | { kind: "stderr"; chunk: string; elapsedMs: number }
  | { kind: "scripted_prompt"; step: number; total: number; text: string; elapsedMs: number }
  | { kind: "idle_warning"; secondsSinceLastEvent: number };

export interface RuntimeRunRequest {
  skill: SkillRecord;
  scenario: Scenario;
  cwd: string;
  /**
   * Ordered list of scripted user turns. The first entry kicks off the
   * session; each subsequent entry is delivered after the agent finishes
   * its previous turn (end_turn). Always contains at least one entry.
   */
  userMessages: string[];
  skillInstallRelPath: string | null;
  userResponses: UserResponseEntry[];
  onProgress?: (event: ProgressEvent) => void;
  idleWarnSeconds?: number;
}

export interface RuntimeRunResult {
  turns: Turn[];
  finalOutput: string;
  turnsUsed: number;
  /** Effective hard cap actually applied. */
  maxTurnsEffective: number;
  /** True if scenario.max_turns was explicitly set. */
  maxTurnsUserSet: boolean;
  sessionId: string | null;
  cost: {
    inputTokens: number;
    outputTokens: number;
    cacheCreationTokens: number;
    cacheReadTokens: number;
    usdEstimate: number;
  };
  unansweredQuestions: number;
  stoppedReason: "end_turn" | "max_turns" | "error" | "other";
  errors: Array<{ kind: string; message: string }>;
}

export interface RuntimeAdapter {
  /** Registry key — `claude`, `codex`, etc. */
  name: string;
  /** Short human-readable description for dry-run output. */
  description: string;
  /** Check that the runtime is usable (CLI present, SDK importable, etc.). */
  preflight(): Promise<{ ok: true } | { ok: false; message: string }>;
  /** Execute one scenario against this runtime. */
  run(req: RuntimeRunRequest): Promise<RuntimeRunResult>;
}

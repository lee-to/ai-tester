import { query } from "@anthropic-ai/claude-agent-sdk";
import type { SDKUserMessage, Options } from "@anthropic-ai/claude-agent-sdk";
import type { Scenario } from "../scenario/schema.js";
import type { SkillRecord, Turn, ToolCallRecord } from "../types.js";
import { compilePattern } from "../util/regex.js";
import { INTERNAL_MAX_TURNS } from "../config.js";
import type { ProgressEvent, RuntimeRunResult } from "../runtimes/types.js";

type AnyMessage = Record<string, unknown> & { type?: string };

export type { ProgressEvent };

export interface SdkRunParams {
  skill: SkillRecord;
  scenario: Scenario;
  cwd: string;
  firstUserMessage: string;
  skillInstallRelPath?: string | null;
  onProgress?: (event: ProgressEvent) => void;
  idleWarnSeconds?: number;
}

export type SdkRunResult = RuntimeRunResult;

export async function runWithSdk(params: SdkRunParams): Promise<SdkRunResult> {
  const { skill, scenario, cwd, firstUserMessage, onProgress } = params;
  const startedAtMs = Date.now();
  const emit = (event: ProgressEvent) => {
    lastEventMs = Date.now();
    onProgress?.(event);
  };
  let lastEventMs = startedAtMs;

  const idleWarnMs = Math.max(5_000, (params.idleWarnSeconds ?? 30) * 1000);
  const idleTimer = setInterval(() => {
    const idleMs = Date.now() - lastEventMs;
    if (idleMs >= idleWarnMs) {
      onProgress?.({
        kind: "idle_warning",
        secondsSinceLastEvent: Math.round(idleMs / 1000),
      });
    }
  }, idleWarnMs);

  const queue = new MessageQueue();
  queue.push(plainUserMessage(firstUserMessage));

  const appendedPrompt = buildSystemPromptAppend(skill, params.skillInstallRelPath);

  const maxTurnsUserSet = typeof scenario.max_turns === "number";
  const maxTurnsEffective = scenario.max_turns ?? INTERNAL_MAX_TURNS;

  const options: Options = {
    cwd,
    // Append the skill body to Claude Code's built-in system prompt rather than
    // replacing it — CC's prompt knows about its tools, permission flow, etc.
    systemPrompt: { type: "preset", preset: "claude_code", append: appendedPrompt },
    permissionMode: scenario.runner.permission_mode as Options["permissionMode"],
    maxTurns: maxTurnsEffective,
    model: scenario.runner.model,
    env: buildEnv(scenario),
    includePartialMessages: false,
    stderr: (chunk: string) => {
      emit({ kind: "stderr", chunk, elapsedMs: Date.now() - startedAtMs });
    },
    ...(skill.allowedToolsRaw.length > 0
      ? { allowedTools: scenario.runner.allowed_tools_override ?? skill.allowedToolsRaw }
      : {}),
  };

  const turns: Turn[] = [];
  const turnById = new Map<string, Turn>();
  const pendingAnswerQueue = [...scenario.user_responses];
  let unansweredQuestions = 0;
  let finalOutput = "";
  let sessionId: string | null = null;
  const errors: SdkRunResult["errors"] = [];
  const cost = {
    inputTokens: 0,
    outputTokens: 0,
    cacheCreationTokens: 0,
    cacheReadTokens: 0,
    usdEstimate: 0,
  };
  let stoppedReason: SdkRunResult["stoppedReason"] = "other";

  const stream = query({ prompt: queue.iterator(), options });

  try {
    for await (const raw of stream) {
      const message = raw as unknown as AnyMessage;
      const elapsed = Date.now() - startedAtMs;
      switch (message.type) {
        case "system":
          if (
            (message as { subtype?: string }).subtype === "init" &&
            typeof message.session_id === "string"
          ) {
            sessionId = message.session_id;
            emit({ kind: "system_init", sessionId, elapsedMs: elapsed });
          }
          break;

        case "assistant":
          handleAssistant(message, turns, turnById, startedAtMs, (ev) => emit(ev));
          maybeAnswerQuestion(
            message,
            pendingAnswerQueue,
            turnById,
            queue,
            (unanswered, info) => {
              if (unanswered) {
                unansweredQuestions++;
                emit({
                  kind: "question_unanswered",
                  tool: info.tool,
                  questionPreview: info.questionPreview,
                  elapsedMs: Date.now() - startedAtMs,
                });
              } else if (info.chosen) {
                emit({
                  kind: "question_answered",
                  tool: info.tool,
                  chosen: info.chosen,
                  questionPreview: info.questionPreview,
                  elapsedMs: Date.now() - startedAtMs,
                });
              }
            }
          );
          break;

        case "user":
          handleUser(message, turnById, startedAtMs, (ev) => emit(ev));
          break;

        case "result":
          handleResult(message, cost, (outputText, reason) => {
            if (outputText) finalOutput = outputText;
            if (reason) stoppedReason = reason;
          });
          emit({
            kind: "result",
            subtype: typeof message.subtype === "string" ? message.subtype : null,
            usdEstimate: cost.usdEstimate,
            elapsedMs: elapsed,
          });
          // Result is terminal — close the prompt queue so the SDK stream can
          // exit cleanly. Without this the iterator stays open and the for-await
          // loop hangs forever.
          queue.close();
          break;

        default:
          break;
      }
    }
  } catch (err) {
    errors.push({ kind: "sdk_stream", message: (err as Error).message });
    stoppedReason = "error";
  } finally {
    clearInterval(idleTimer);
    queue.close();
  }

  if (stoppedReason === "other" && errors.length === 0) stoppedReason = "end_turn";

  return {
    turns,
    finalOutput,
    turnsUsed: turns.filter((t) => t.role === "assistant").length,
    maxTurnsEffective,
    maxTurnsUserSet,
    sessionId,
    cost,
    unansweredQuestions,
    stoppedReason,
    errors,
  };
}

// ---------------------------------------------------------------------------
// Stream handlers
// ---------------------------------------------------------------------------

function handleAssistant(
  message: AnyMessage,
  turns: Turn[],
  turnById: Map<string, Turn>,
  startedAtMs: number,
  emit: (ev: ProgressEvent) => void
): void {
  const content = extractContent(message);
  const toolCalls: ToolCallRecord[] = [];
  const textDeltas: string[] = [];
  for (const block of content) {
    if (block?.type === "text" && typeof block.text === "string") {
      textDeltas.push(block.text);
      if (block.text.trim().length > 0) {
        emit({ kind: "assistant_text", text: block.text, elapsedMs: Date.now() - startedAtMs });
      }
    } else if (block?.type === "tool_use") {
      const tc: ToolCallRecord = {
        id: typeof block.id === "string" ? block.id : "",
        name: typeof block.name === "string" ? block.name : "",
        input: (block.input ?? {}) as Record<string, unknown>,
        resultContent: null,
        resultIsError: false,
        answered: null,
      };
      toolCalls.push(tc);
      if (tc.id) turnById.set(tc.id, { ...EMPTY_TURN });
      emit({
        kind: "tool_use",
        tool: tc.name,
        input: tc.input,
        elapsedMs: Date.now() - startedAtMs,
      });
    }
  }
  const usage = extractUsage(message);
  const turn: Turn = {
    index: turns.length,
    role: "assistant",
    textDeltas,
    toolCalls,
    ...(usage ? { usage } : {}),
  };
  turns.push(turn);
  for (const tc of toolCalls) {
    if (tc.id) turnById.set(tc.id, turn);
  }
}

function handleUser(
  message: AnyMessage,
  turnById: Map<string, Turn>,
  startedAtMs: number,
  emit: (ev: ProgressEvent) => void
): void {
  const content = extractContent(message);
  for (const block of content) {
    if (block?.type !== "tool_result") continue;
    const tool_use_id = typeof block.tool_use_id === "string" ? block.tool_use_id : "";
    if (!tool_use_id) continue;
    const turn = turnById.get(tool_use_id);
    if (!turn?.toolCalls) continue;
    const tc = turn.toolCalls.find((t) => t.id === tool_use_id);
    if (!tc) continue;
    tc.resultContent = stringifyContent(block.content);
    tc.resultIsError = Boolean(block.is_error);
    emit({
      kind: "tool_result",
      tool: tc.name,
      toolUseId: tool_use_id,
      content: tc.resultContent ?? "",
      isError: tc.resultIsError,
      elapsedMs: Date.now() - startedAtMs,
    });
  }
}

function handleResult(
  message: AnyMessage,
  cost: SdkRunResult["cost"],
  finalize: (
    outputText: string | null,
    reason: SdkRunResult["stoppedReason"] | null
  ) => void
): void {
  const directResult = typeof message.result === "string" ? message.result : "";
  finalize(directResult || null, null);
  const usage = (message.usage as Record<string, unknown> | undefined) ?? {};
  cost.inputTokens += num(usage.input_tokens);
  cost.outputTokens += num(usage.output_tokens);
  cost.cacheCreationTokens += num(usage.cache_creation_input_tokens);
  cost.cacheReadTokens += num(usage.cache_read_input_tokens);
  if (typeof message.total_cost_usd === "number") {
    cost.usdEstimate = message.total_cost_usd;
  }
  const subtype = typeof message.subtype === "string" ? message.subtype : "";
  if (subtype === "error_max_turns") {
    finalize(null, "max_turns");
  } else if (subtype === "success") {
    finalize(null, "end_turn");
  }
}

function maybeAnswerQuestion(
  message: AnyMessage,
  pending: Array<{ match_question: string; choose: string }>,
  turnById: Map<string, Turn>,
  queue: MessageQueue,
  report: (
    unanswered: boolean,
    info: { tool: string; questionPreview: string; chosen?: string }
  ) => void
): void {
  const content = extractContent(message);
  for (const block of content) {
    if (block?.type !== "tool_use") continue;
    if (block.name !== "AskUserQuestion" && block.name !== "Questions") continue;
    const toolUseId = typeof block.id === "string" ? block.id : "";
    const toolName = typeof block.name === "string" ? block.name : "";
    const input = (block.input ?? {}) as Record<string, unknown>;

    // Claude's AskUserQuestion can batch multiple questions in one call
    // (input.questions[]). Match EACH question separately against user_responses
    // so an unanswered question in a batch is surfaced even if its sibling
    // questions had matches.
    const questions = splitIntoQuestions(input);
    const answers: Array<{ question: string; answer: string | null }> = [];
    for (const q of questions) {
      const preview = q.slice(0, 100);
      const match = pickAnswerForQuestion(q, pending);
      if (!match) {
        report(true, { tool: toolName, questionPreview: preview });
        answers.push({ question: q, answer: null });
        continue;
      }
      pending.splice(match.index, 1);
      report(false, { tool: toolName, questionPreview: preview, chosen: match.label });
      answers.push({ question: q, answer: match.label });
    }

    // Annotate the trace with whatever we ended up answering.
    const turn = toolUseId ? turnById.get(toolUseId) : undefined;
    const tc = turn?.toolCalls?.find((t) => t.id === toolUseId);
    if (tc) {
      const firstAnswered = answers.find((a) => a.answer !== null);
      if (firstAnswered) {
        tc.answered = {
          matchedEntryIndex: -1,
          chosenLabel: answers
            .map((a) => `${a.question}: ${a.answer ?? "[no match]"}`)
            .join(" | "),
        };
      }
    }

    queue.push(toolResultMessage(toolUseId, formatBatchAnswer(answers)));
  }
}

function splitIntoQuestions(input: Record<string, unknown>): string[] {
  if (Array.isArray(input.questions)) {
    return (input.questions as unknown[])
      .map((q) =>
        q && typeof q === "object" ? String((q as Record<string, unknown>).question ?? "") : ""
      )
      .filter((q) => q.length > 0);
  }
  if (typeof input.question === "string" && input.question.length > 0) {
    return [input.question];
  }
  return [];
}

function pickAnswerForQuestion(
  question: string,
  pending: Array<{ match_question: string; choose: string }>
): { index: number; label: string } | null {
  for (let i = 0; i < pending.length; i++) {
    const entry = pending[i]!;
    let re: RegExp;
    try {
      re = compilePattern(entry.match_question);
    } catch {
      continue;
    }
    if (re.test(question)) {
      return { index: i, label: entry.choose };
    }
  }
  return null;
}

function formatBatchAnswer(
  answers: Array<{ question: string; answer: string | null }>
): string {
  if (answers.length === 0) return "No questions received.";
  if (answers.length === 1) {
    const a = answers[0]!;
    return a.answer ?? "No pre-registered answer for this question.";
  }
  return answers
    .map((a, i) =>
      `Q${i + 1} "${a.question}": ${a.answer ?? "[no pre-registered answer]"}`
    )
    .join("\n");
}


// ---------------------------------------------------------------------------
// Queue + message helpers
// ---------------------------------------------------------------------------

class MessageQueue {
  private queue: SDKUserMessage[] = [];
  private resolvers: Array<(v: IteratorResult<SDKUserMessage>) => void> = [];
  private closed = false;

  push(msg: SDKUserMessage): void {
    if (this.closed) return;
    const next = this.resolvers.shift();
    if (next) {
      next({ value: msg, done: false });
    } else {
      this.queue.push(msg);
    }
  }

  close(): void {
    if (this.closed) return;
    this.closed = true;
    for (const r of this.resolvers) r({ value: undefined as unknown as SDKUserMessage, done: true });
    this.resolvers = [];
  }

  async *iterator(): AsyncGenerator<SDKUserMessage> {
    while (true) {
      if (this.queue.length > 0) {
        yield this.queue.shift()!;
        continue;
      }
      if (this.closed) return;
      const result = await new Promise<IteratorResult<SDKUserMessage>>((resolve) => {
        this.resolvers.push(resolve);
      });
      if (result.done) return;
      yield result.value;
    }
  }
}

function buildSystemPromptAppend(
  skill: SkillRecord,
  skillInstallRelPath: string | null | undefined
): string {
  const body = skill.body;
  if (!skillInstallRelPath) return body;
  const refPath = `${skillInstallRelPath}/references/`;
  const note =
    `\n\n---\n\n` +
    `## Skill installation context (ai-tester)\n\n` +
    `This skill is installed at \`${skillInstallRelPath}/\` relative to the current working directory. ` +
    `When the instructions above refer to relative paths like \`references/FOO.md\`, read them from ` +
    `\`${refPath}FOO.md\` — use the Read tool with that path. Do NOT try to Read the bare relative ` +
    `path; it will fail because \`cwd\` is the target project, not the skill directory.\n`;
  return body + note;
}

function plainUserMessage(text: string): SDKUserMessage {
  return {
    type: "user",
    message: { role: "user", content: text },
    parent_tool_use_id: null,
  };
}

function toolResultMessage(toolUseId: string, text: string): SDKUserMessage {
  return {
    type: "user",
    message: {
      role: "user",
      content: [
        {
          type: "tool_result",
          tool_use_id: toolUseId,
          content: text,
          is_error: false,
        },
      ],
    },
    parent_tool_use_id: toolUseId || null,
  };
}

function buildEnv(_scenario: Scenario): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(process.env)) {
    if (!v) continue;
    if (
      k.startsWith("CLAUDE_") ||
      k.startsWith("ANTHROPIC_") ||
      k === "HOME" ||
      k === "PATH" ||
      k === "SHELL" ||
      k === "USER" ||
      k === "TMPDIR" ||
      k === "LANG" ||
      k === "NODE_ENV"
    ) {
      out[k] = v;
    }
  }
  return out;
}

// ---------------------------------------------------------------------------
// SDK message helpers (typed loosely — SDK message shapes vary)
// ---------------------------------------------------------------------------

function extractContent(message: AnyMessage): Array<Record<string, unknown>> {
  const inner = message.message as { content?: unknown } | undefined;
  const raw = inner?.content;
  if (!Array.isArray(raw)) return [];
  return raw.filter((b): b is Record<string, unknown> => !!b && typeof b === "object");
}

function extractUsage(message: AnyMessage): Turn["usage"] | null {
  const inner = message.message as { usage?: Record<string, unknown> } | undefined;
  const u = inner?.usage;
  if (!u) return null;
  return {
    inputTokens: num(u.input_tokens),
    cacheCreationInputTokens: num(u.cache_creation_input_tokens),
    cacheReadInputTokens: num(u.cache_read_input_tokens),
    outputTokens: num(u.output_tokens),
  };
}

function stringifyContent(content: unknown): string {
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    return content
      .map((b) => {
        if (b && typeof b === "object" && (b as { type?: string }).type === "text") {
          return String((b as { text?: unknown }).text ?? "");
        }
        return JSON.stringify(b);
      })
      .join("\n");
  }
  if (content == null) return "";
  return JSON.stringify(content);
}

function num(v: unknown): number {
  return typeof v === "number" && Number.isFinite(v) ? v : 0;
}

const EMPTY_TURN: Turn = {
  index: -1,
  role: "assistant",
};

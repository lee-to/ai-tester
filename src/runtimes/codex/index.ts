import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { Codex } from "@openai/codex-sdk";
import type {
  ThreadEvent,
  ThreadItem,
  ThreadOptions,
  SandboxMode,
  ApprovalMode,
} from "@openai/codex-sdk";
import type {
  ProgressEvent,
  RuntimeAdapter,
  RuntimeRunRequest,
  RuntimeRunResult,
} from "../types.js";
import type { Turn, ToolCallRecord } from "../../types.js";
import { INTERNAL_MAX_TURNS } from "../../config.js";

const exec = promisify(execFile);

export function createCodexRuntime(): RuntimeAdapter {
  return {
    name: "codex",
    description:
      "OpenAI Codex via @openai/codex-sdk. Spawns the `codex` CLI — if you are " +
      "logged in via `codex login` the ChatGPT subscription is used; otherwise " +
      "falls back to OPENAI_API_KEY.",

    async preflight() {
      try {
        const { stdout } = await exec("which", ["codex"]);
        if (!stdout.trim()) throw new Error("empty");
        return { ok: true as const };
      } catch {
        return {
          ok: false as const,
          message:
            "`codex` CLI not found on PATH. Install it from https://github.com/openai/codex " +
            "and sign in with `codex login`.",
        };
      }
    },

    async run(req: RuntimeRunRequest): Promise<RuntimeRunResult> {
      return runCodex(req);
    },
  };
}

async function runCodex(req: RuntimeRunRequest): Promise<RuntimeRunResult> {
  const { skill, scenario, cwd, userMessages, onProgress } = req;
  if (userMessages.length === 0) {
    throw new Error("codex runtime: userMessages must contain at least one entry");
  }

  const startedAtMs = Date.now();
  let lastEventMs = startedAtMs;
  const emit = (event: ProgressEvent): void => {
    lastEventMs = Date.now();
    onProgress?.(event);
  };

  const idleWarnMs = Math.max(5_000, (req.idleWarnSeconds ?? 30) * 1000);
  const idleTimer = setInterval(() => {
    const idleMs = Date.now() - lastEventMs;
    if (idleMs >= idleWarnMs) {
      onProgress?.({
        kind: "idle_warning",
        secondsSinceLastEvent: Math.round(idleMs / 1000),
      });
    }
  }, idleWarnMs);

  const maxTurnsUserSet = typeof scenario.max_turns === "number";
  const maxTurnsEffective = scenario.max_turns ?? INTERNAL_MAX_TURNS;

  const threadOptions: ThreadOptions = {
    workingDirectory: cwd,
    sandboxMode: mapPermissionMode(scenario.runner.permission_mode),
    approvalPolicy: "never" satisfies ApprovalMode,
    skipGitRepoCheck: true,
    ...(scenario.runner.model ? { model: scenario.runner.model } : {}),
  };

  const codex = new Codex({ env: buildCodexEnv() });
  const thread = codex.startThread(threadOptions);

  const turns: Turn[] = [];
  let currentTurn: Turn | null = null;
  const cost = {
    inputTokens: 0,
    outputTokens: 0,
    cacheCreationTokens: 0,
    cacheReadTokens: 0,
    usdEstimate: 0,
  };
  let finalOutput = "";
  let sessionId: string | null = null;
  let turnCount = 0;
  const errors: RuntimeRunResult["errors"] = [];
  let stoppedReason: RuntimeRunResult["stoppedReason"] = "other";

  const abort = new AbortController();

  try {
    for (let i = 0; i < userMessages.length; i++) {
      // First scripted turn folds the skill body in as Codex has no separate
      // system prompt. Subsequent turns are sent plain — the thread already
      // has the skill context from the first message.
      const input =
        i === 0
          ? buildCodexInput(skill.body, req.skillInstallRelPath, userMessages[0])
          : userMessages[i];
      if (userMessages.length > 1) {
        emit({
          kind: "scripted_prompt",
          step: i + 1,
          total: userMessages.length,
          text: userMessages[i],
          elapsedMs: Date.now() - startedAtMs,
        });
      }
      const streamed = await thread.runStreamed(input, { signal: abort.signal });
      for await (const event of streamed.events as AsyncGenerator<ThreadEvent>) {
        const elapsed = Date.now() - startedAtMs;
        switch (event.type) {
          case "thread.started":
            sessionId = event.thread_id;
            emit({ kind: "system_init", sessionId, elapsedMs: elapsed });
            break;

          case "turn.started":
            turnCount++;
            currentTurn = {
              index: turns.length,
              role: "assistant",
              textDeltas: [],
              toolCalls: [],
            };
            turns.push(currentTurn);
            if (turnCount > maxTurnsEffective) {
              stoppedReason = "max_turns";
              abort.abort();
            }
            break;

          case "item.started":
            handleItem(event.item, false, currentTurn, emit, startedAtMs);
            break;

          case "item.completed":
            handleItem(event.item, true, currentTurn, emit, startedAtMs);
            if (event.item.type === "agent_message") {
              finalOutput = event.item.text;
            }
            break;

          case "turn.completed":
            if (event.usage) {
              cost.inputTokens += event.usage.input_tokens;
              cost.outputTokens += event.usage.output_tokens;
              cost.cacheReadTokens += event.usage.cached_input_tokens;
            }
            emit({
              kind: "result",
              subtype: "success",
              usdEstimate: cost.usdEstimate,
              elapsedMs: elapsed,
            });
            break;

          case "turn.failed":
            errors.push({ kind: "codex_turn_failed", message: event.error.message });
            stoppedReason = "error";
            emit({
              kind: "result",
              subtype: "error",
              usdEstimate: cost.usdEstimate,
              elapsedMs: elapsed,
            });
            break;

          case "error":
            errors.push({ kind: "codex_stream_error", message: event.message });
            stoppedReason = "error";
            break;

          default:
            break;
        }
      }
      if (stoppedReason === "error" || stoppedReason === "max_turns") break;
    }
  } catch (err) {
    if ((err as Error).name !== "AbortError") {
      errors.push({ kind: "codex_runner", message: (err as Error).message });
      stoppedReason = "error";
    }
  } finally {
    clearInterval(idleTimer);
  }

  if (stoppedReason === "other" && errors.length === 0) stoppedReason = "end_turn";

  // user_responses aren't consumable in Codex — it has no AskUserQuestion equivalent.
  // Count pending entries as "unanswered" only if scenarios expected them. Actually:
  // we simply report zero here — Codex tests shouldn't rely on user_responses.
  const unansweredQuestions = 0;

  return {
    turns,
    finalOutput,
    turnsUsed: turnCount,
    maxTurnsEffective,
    maxTurnsUserSet,
    sessionId,
    cost,
    unansweredQuestions,
    stoppedReason,
    errors,
  };
}

function handleItem(
  item: ThreadItem,
  isCompleted: boolean,
  currentTurn: Turn | null,
  emit: (ev: ProgressEvent) => void,
  startedAtMs: number
): void {
  const elapsed = Date.now() - startedAtMs;
  switch (item.type) {
    case "command_execution": {
      if (!isCompleted) {
        const tc: ToolCallRecord = {
          id: item.id,
          name: "Bash",
          input: { command: item.command },
          resultContent: null,
          resultIsError: false,
          answered: null,
        };
        currentTurn?.toolCalls?.push(tc);
        emit({
          kind: "tool_use",
          tool: "Bash",
          input: { command: item.command },
          elapsedMs: elapsed,
        });
        return;
      }
      const tc = currentTurn?.toolCalls?.find((t) => t.id === item.id);
      if (tc) {
        tc.resultContent = item.aggregated_output;
        tc.resultIsError = item.status === "failed";
      }
      emit({
        kind: "tool_result",
        tool: "Bash",
        toolUseId: item.id,
        content: item.aggregated_output,
        isError: item.status === "failed",
        elapsedMs: elapsed,
      });
      return;
    }

    case "file_change": {
      if (!isCompleted) return;
      for (const change of item.changes) {
        const toolName =
          change.kind === "add" ? "Write" : change.kind === "delete" ? "Bash" : "Edit";
        const input: Record<string, unknown> =
          change.kind === "delete"
            ? { command: `rm ${change.path}` }
            : { file_path: change.path };
        const tc: ToolCallRecord = {
          id: `${item.id}:${change.path}`,
          name: toolName,
          input,
          resultContent: `${change.kind} ${change.path}`,
          resultIsError: item.status === "failed",
          answered: null,
        };
        currentTurn?.toolCalls?.push(tc);
        emit({ kind: "tool_use", tool: toolName, input, elapsedMs: elapsed });
        emit({
          kind: "tool_result",
          tool: toolName,
          toolUseId: tc.id,
          content: tc.resultContent ?? "",
          isError: tc.resultIsError,
          elapsedMs: elapsed,
        });
      }
      return;
    }

    case "mcp_tool_call": {
      if (!isCompleted) return;
      const toolName = `mcp__${item.server}__${item.tool}`;
      const input = (item.arguments ?? {}) as Record<string, unknown>;
      const content = item.result
        ? JSON.stringify(item.result)
        : item.error?.message ?? "";
      const tc: ToolCallRecord = {
        id: item.id,
        name: toolName,
        input,
        resultContent: content,
        resultIsError: item.status === "failed",
        answered: null,
      };
      currentTurn?.toolCalls?.push(tc);
      emit({ kind: "tool_use", tool: toolName, input, elapsedMs: elapsed });
      emit({
        kind: "tool_result",
        tool: toolName,
        toolUseId: item.id,
        content,
        isError: tc.resultIsError,
        elapsedMs: elapsed,
      });
      return;
    }

    case "web_search": {
      if (!isCompleted) return;
      const input = { query: item.query };
      const tc: ToolCallRecord = {
        id: item.id,
        name: "WebSearch",
        input,
        resultContent: null,
        resultIsError: false,
        answered: null,
      };
      currentTurn?.toolCalls?.push(tc);
      emit({ kind: "tool_use", tool: "WebSearch", input, elapsedMs: elapsed });
      return;
    }

    case "agent_message": {
      if (!isCompleted) return;
      currentTurn?.textDeltas?.push(item.text);
      emit({ kind: "assistant_text", text: item.text, elapsedMs: elapsed });
      return;
    }

    case "error": {
      if (!isCompleted) return;
      emit({ kind: "stderr", chunk: `[error item] ${item.message}`, elapsedMs: elapsed });
      return;
    }

    default:
      return;
  }
}

function mapPermissionMode(mode: string): SandboxMode {
  switch (mode) {
    case "bypassPermissions":
      return "danger-full-access";
    case "acceptEdits":
      return "workspace-write";
    case "plan":
      return "read-only";
    default:
      return "workspace-write";
  }
}

function buildCodexInput(
  skillBody: string,
  skillInstallRelPath: string | null,
  userMessage: string
): string {
  const parts: string[] = [];
  parts.push(skillBody);
  if (skillInstallRelPath) {
    parts.push(
      `---\n\n## Skill installation context (ai-tester)\n\n` +
        `This skill is installed at \`${skillInstallRelPath}/\` relative to the current ` +
        `working directory. When the instructions above refer to relative paths like ` +
        `\`references/FOO.md\`, read them from \`${skillInstallRelPath}/references/FOO.md\`.`
    );
  }
  parts.push(`---\n\n## User request\n\n${userMessage}`);
  return parts.join("\n\n");
}

function buildCodexEnv(): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(process.env)) {
    if (!v) continue;
    if (
      k.startsWith("OPENAI_") ||
      k.startsWith("CODEX_") ||
      k === "HOME" ||
      k === "PATH" ||
      k === "SHELL" ||
      k === "USER" ||
      k === "TMPDIR" ||
      k === "LANG" ||
      k === "NODE_ENV" ||
      k === "TERM"
    ) {
      out[k] = v;
    }
  }
  return out;
}

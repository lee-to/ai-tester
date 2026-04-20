import { execFile } from "node:child_process";
import { promisify } from "node:util";
import type {
  RuntimeAdapter,
  RuntimeRunRequest,
  RuntimeRunResult,
} from "../types.js";
import { runWithSdk } from "../../runner/sdk-runner.js";

const exec = promisify(execFile);

export function createClaudeRuntime(): RuntimeAdapter {
  return {
    name: "claude",
    description:
      "Claude Code via @anthropic-ai/claude-agent-sdk. Uses the logged-in " +
      "`claude` CLI OAuth session — bills against your Claude subscription.",

    async preflight() {
      try {
        const { stdout } = await exec("which", ["claude"]);
        if (!stdout.trim()) throw new Error("empty");
        return { ok: true as const };
      } catch {
        return {
          ok: false as const,
          message:
            "`claude` CLI not found on PATH. Install Claude Code and sign in with " +
            "`claude login` so the Claude Agent SDK can reuse its OAuth session.",
        };
      }
    },

    async run(req: RuntimeRunRequest): Promise<RuntimeRunResult> {
      return runWithSdk({
        skill: req.skill,
        scenario: req.scenario,
        cwd: req.cwd,
        firstUserMessage: req.firstUserMessage,
        skillInstallRelPath: req.skillInstallRelPath,
        onProgress: req.onProgress,
        idleWarnSeconds: req.idleWarnSeconds,
      });
    },
  };
}

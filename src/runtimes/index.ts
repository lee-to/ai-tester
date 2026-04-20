import { registerRuntime, getRuntime, listRuntimes, hasRuntime } from "./registry.js";
import { createClaudeRuntime } from "./claude/index.js";
import { createCodexRuntime } from "./codex/index.js";

let bootstrapped = false;

/** Register all built-in runtimes. Idempotent. */
export function bootstrapRuntimes(): void {
  if (bootstrapped) return;
  registerRuntime(createClaudeRuntime());
  registerRuntime(createCodexRuntime());
  bootstrapped = true;
}

export { getRuntime, listRuntimes, hasRuntime, registerRuntime };
export type { RuntimeAdapter, RuntimeRunRequest, RuntimeRunResult, ProgressEvent } from "./types.js";

export const DEFAULT_MODEL = "claude-sonnet-4-6";
/**
 * Internal safety cap when the scenario does not declare its own `max_turns`.
 * Hitting it does NOT fail the scenario — it emits a yellow warning. Only an
 * explicit user-set `max_turns` failing is treated as a test failure.
 */
export const INTERNAL_MAX_TURNS = 40;
export const DEFAULT_PASS_THRESHOLD = 0.85;

export function getApiKey(): string | null {
  return process.env.ANTHROPIC_API_KEY?.trim() || null;
}

export function requireApiKey(): string {
  const key = getApiKey();
  if (!key) {
    throw new Error(
      "ANTHROPIC_API_KEY is not set. Set it in your environment before running scenarios.\n" +
        "  export ANTHROPIC_API_KEY=sk-ant-..."
    );
  }
  return key;
}

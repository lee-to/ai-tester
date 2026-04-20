/**
 * Tracks pending sandbox cleanup functions so a SIGINT/SIGTERM/SIGHUP doesn't
 * leave orphan worktrees under $TMPDIR. Registrations are removed on normal
 * completion by calling the untrack handle returned from `trackCleanup`.
 */

type CleanupFn = () => Promise<void>;

const pending = new Set<CleanupFn>();
let handlersInstalled = false;
const SIGNAL_CLEANUP_TIMEOUT_MS = 3_000;

function installSignalHandlers(): void {
  if (handlersInstalled) return;
  handlersInstalled = true;

  const make = (signal: NodeJS.Signals, exitCode: number) =>
    async function handler(): Promise<void> {
      if (pending.size === 0) {
        process.exit(exitCode);
      }
      // Can't use console.log — may race with other tool output. Go straight
      // to stderr so the message is visible even with --quiet.
      process.stderr.write(
        `\n[ai-tester] ${signal} received — removing ${pending.size} sandbox(es)…\n`
      );
      await runAll(SIGNAL_CLEANUP_TIMEOUT_MS);
      process.exit(exitCode);
    };

  process.once("SIGINT", make("SIGINT", 130));
  process.once("SIGTERM", make("SIGTERM", 143));
  process.once("SIGHUP", make("SIGHUP", 129));
}

async function runAll(timeoutMs: number): Promise<void> {
  const jobs = [...pending].map((fn) =>
    fn().catch((err: unknown) => {
      process.stderr.write(
        `[ai-tester] cleanup error: ${(err as Error).message ?? String(err)}\n`
      );
    })
  );
  pending.clear();
  await Promise.race([
    Promise.allSettled(jobs),
    new Promise<void>((resolve) => setTimeout(resolve, timeoutMs)),
  ]);
}

export function trackCleanup(fn: CleanupFn): () => void {
  installSignalHandlers();
  pending.add(fn);
  return () => {
    pending.delete(fn);
  };
}

export function pendingCleanupCount(): number {
  return pending.size;
}

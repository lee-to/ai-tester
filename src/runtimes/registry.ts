import type { RuntimeAdapter } from "./types.js";

const registry = new Map<string, RuntimeAdapter>();

export function registerRuntime(adapter: RuntimeAdapter): void {
  registry.set(adapter.name, adapter);
}

export function getRuntime(name: string): RuntimeAdapter {
  const adapter = registry.get(name);
  if (!adapter) {
    const known = [...registry.keys()].join(", ") || "(none registered)";
    throw new Error(`Unknown runtime "${name}". Known runtimes: ${known}`);
  }
  return adapter;
}

export function hasRuntime(name: string): boolean {
  return registry.has(name);
}

export function listRuntimes(): RuntimeAdapter[] {
  return [...registry.values()];
}

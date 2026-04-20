import { test } from "node:test";
import assert from "node:assert/strict";
import {
  tokenizeAllowedTools,
  parseAllowedTools,
} from "../../dist/skill/allowed-tools.js";

test("tokenizeAllowedTools: empty / null / undefined → empty result", () => {
  assert.deepEqual(tokenizeAllowedTools(""), { raw: [], parsed: [] });
  assert.deepEqual(tokenizeAllowedTools(null), { raw: [], parsed: [] });
  assert.deepEqual(tokenizeAllowedTools(undefined), { raw: [], parsed: [] });
});

test("tokenizeAllowedTools: simple whitespace-separated names", () => {
  const out = tokenizeAllowedTools("Read Write Bash");
  assert.deepEqual(out.raw, ["Read", "Write", "Bash"]);
  assert.deepEqual(
    out.parsed.map((p) => p.name).sort(),
    ["Bash", "Read", "Write"]
  );
  for (const p of out.parsed) assert.deepEqual(p.scopes, []);
});

test("tokenizeAllowedTools: tool with single scope", () => {
  const out = tokenizeAllowedTools("Bash(git *)");
  assert.deepEqual(out.raw, ["Bash(git *)"]);
  assert.equal(out.parsed.length, 1);
  assert.equal(out.parsed[0].name, "Bash");
  assert.deepEqual(out.parsed[0].scopes, ["git *"]);
});

test("tokenizeAllowedTools: comma-separated scopes inside parens", () => {
  const out = tokenizeAllowedTools("Bash(mkdir, npx, python)");
  assert.equal(out.parsed.length, 1);
  assert.equal(out.parsed[0].name, "Bash");
  assert.deepEqual(out.parsed[0].scopes.sort(), ["mkdir", "npx", "python"]);
});

test("tokenizeAllowedTools: duplicate tools merge their scopes", () => {
  const out = tokenizeAllowedTools("Bash(mkdir) Bash(git *)");
  assert.equal(out.parsed.length, 1);
  assert.equal(out.parsed[0].name, "Bash");
  assert.deepEqual(out.parsed[0].scopes.sort(), ["git *", "mkdir"]);
});

test("tokenizeAllowedTools: respects parens — whitespace inside scopes is not a separator", () => {
  const out = tokenizeAllowedTools("Bash(git commit -m)");
  assert.deepEqual(out.raw, ["Bash(git commit -m)"]);
  assert.deepEqual(out.parsed[0].scopes, ["git commit -m"]);
});

test("tokenizeAllowedTools: MCP-style long tool names", () => {
  const out = tokenizeAllowedTools("mcp__handoff__handoff_sync_status Read");
  assert.equal(out.parsed.length, 2);
  const names = out.parsed.map((p) => p.name).sort();
  assert.deepEqual(names, ["Read", "mcp__handoff__handoff_sync_status"]);
});

test("parseAllowedTools: backwards-compat wrapper returns .parsed only", () => {
  const parsed = parseAllowedTools("Read Bash(git *)");
  assert.equal(parsed.length, 2);
  assert.ok(parsed.every((p) => typeof p.name === "string"));
  assert.ok(parsed.every((p) => Array.isArray(p.scopes)));
});

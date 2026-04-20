import { test } from "node:test";
import assert from "node:assert/strict";
import { compilePattern } from "../../dist/util/regex.js";

test("compilePattern: plain pattern without flags", () => {
  const re = compilePattern("foo");
  assert.equal(re.flags, "");
  assert.ok(re.test("foo"));
  assert.ok(!re.test("FOO"));
});

test("compilePattern: (?i) lifted to case-insensitive flag", () => {
  const re = compilePattern("(?i)foo");
  assert.equal(re.flags, "i");
  assert.ok(re.test("FOO"));
  assert.ok(re.test("Foo"));
});

test("compilePattern: (?ims) lifts multiple flags", () => {
  const re = compilePattern("(?ims)foo.bar");
  assert.ok(re.flags.includes("i"));
  assert.ok(re.flags.includes("m"));
  assert.ok(re.flags.includes("s"));
  assert.ok(re.test("foo\nbar"));
});

test("compilePattern: unknown inline flags are dropped (only ims kept)", () => {
  const re = compilePattern("(?ix)foo");
  assert.equal(re.flags, "i");
});

test("compilePattern: inline flag block stripped from source", () => {
  const re = compilePattern("(?i)^hello$");
  assert.ok(re.test("HELLO"));
  assert.ok(!re.test("xHELLOx"));
});

test("compilePattern: non-inline parentheses untouched", () => {
  const re = compilePattern("(foo|bar)");
  assert.ok(re.test("foo"));
  assert.ok(re.test("bar"));
  assert.ok(!re.test("baz"));
});

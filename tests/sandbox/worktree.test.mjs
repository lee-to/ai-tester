import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import { createSandbox } from "../../dist/sandbox/worktree.js";

const emptyFixtures = {
  git_init: false,
  copy_trees: [],
  files_committed: [],
  files_staged: [],
  files_unstaged: [],
  setup_commands: [],
  env: {},
};

test("createSandbox: creates a temp dir under tmpdir with the scenario name", async () => {
  const sb = await createSandbox("scenario-name", emptyFixtures);
  try {
    const stat = await fs.stat(sb.path);
    assert.ok(stat.isDirectory());
    assert.ok(path.basename(sb.path).startsWith("ai-tester-scenario-name-"));
    assert.equal(sb.skillInstallPath, null);
  } finally {
    await sb.cleanup();
  }
});

test("createSandbox: cleanup removes the directory", async () => {
  const sb = await createSandbox("cleanup-check", emptyFixtures);
  await sb.cleanup();
  await assert.rejects(() => fs.stat(sb.path));
});

test("createSandbox: files_unstaged are written to disk without git", async () => {
  const sb = await createSandbox("unstaged-files", {
    ...emptyFixtures,
    files_unstaged: [
      { path: "TODO.md", content: "- audit\n" },
      { path: "nested/dir/note.txt", content: "hello" },
    ],
  });
  try {
    const todo = await fs.readFile(path.join(sb.path, "TODO.md"), "utf-8");
    assert.equal(todo, "- audit\n");
    const nested = await fs.readFile(
      path.join(sb.path, "nested/dir/note.txt"),
      "utf-8"
    );
    assert.equal(nested, "hello");
  } finally {
    await sb.cleanup();
  }
});

test("createSandbox: git_init creates a working repo with empty commit when no content", async () => {
  const sb = await createSandbox("git-empty", {
    ...emptyFixtures,
    git_init: true,
  });
  try {
    const gitDir = await fs.stat(path.join(sb.path, ".git"));
    assert.ok(gitDir.isDirectory());
    // Sentinel file for the initial empty commit.
    const sentinel = await fs.stat(
      path.join(sb.path, ".ai-tester-keep")
    );
    assert.ok(sentinel.isFile());
  } finally {
    await sb.cleanup();
  }
});

test("createSandbox: files_committed are written as initial baseline", async () => {
  const sb = await createSandbox("git-baseline", {
    ...emptyFixtures,
    git_init: true,
    files_committed: [{ path: "README.md", content: "# Demo\n" }],
  });
  try {
    const content = await fs.readFile(
      path.join(sb.path, "README.md"),
      "utf-8"
    );
    assert.equal(content, "# Demo\n");
  } finally {
    await sb.cleanup();
  }
});

test("createSandbox: fixture path attempting escape is rejected", async () => {
  await assert.rejects(
    () =>
      createSandbox("escape", {
        ...emptyFixtures,
        files_unstaged: [{ path: "../escape.txt", content: "nope" }],
      }),
    /escapes sandbox/
  );
});

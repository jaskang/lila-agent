import { expect, test } from "vitest";
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { gitCommit, gitCreateWorktree, gitDiff, gitDiscard, gitStage, gitStatus, gitUnstage } from "./git.ts";

function git(cwd: string, args: string[]): void {
  execFileSync("git", args, { cwd, stdio: "ignore" });
}

test("reports non-repository directories cleanly", async () => {
  const dir = mkdtempSync(join(tmpdir(), "lila-nogit-"));
  expect(await gitStatus(dir)).toEqual({ isRepo: false, files: [] });
});

test("lists staged, unstaged, and untracked changes with diffs", async () => {
  const dir = mkdtempSync(join(tmpdir(), "lila-git-"));
  git(dir, ["init", "-b", "main"]);
  git(dir, ["config", "user.email", "test@example.com"]);
  git(dir, ["config", "user.name", "Test"]);
  writeFileSync(join(dir, "a.ts"), "export const a = 1;\n");
  git(dir, ["add", "a.ts"]);
  git(dir, ["commit", "-m", "init"]);

  writeFileSync(join(dir, "a.ts"), "export const a = 2;\nexport const b = 3;\n");
  writeFileSync(join(dir, "new file.ts"), "console.log('hi');\n");
  writeFileSync(join(dir, "staged.ts"), "export const s = true;\n");
  git(dir, ["add", "staged.ts"]);

  const status = await gitStatus(dir);
  expect(status.isRepo).toBe(true);
  expect(status.branch).toBe("main");
  expect(status.files).toEqual([
    { path: "a.ts", status: "modified", staged: false, unstaged: true, added: 2, removed: 1 },
    { path: "new file.ts", status: "untracked", staged: false, unstaged: true },
    { path: "staged.ts", status: "added", staged: true, unstaged: false, added: 1, removed: 0 },
  ]);

  const modified = await gitDiff(dir, "a.ts");
  expect(modified.patch).toContain("-export const a = 1;");
  expect(modified.patch).toContain("+export const a = 2;");

  const untracked = await gitDiff(dir, "new file.ts");
  expect(untracked.patch).toContain("+++ b/new file.ts");
  expect(untracked.patch).toContain("+console.log('hi');");

  await expect(gitDiff(dir, "../outside.ts")).rejects.toThrow("不在项目目录内");
});

test("does not follow untracked symlinks pointing outside the repository", async () => {
  const outsideDir = mkdtempSync(join(tmpdir(), "lila-secret-"));
  const secretPath = join(outsideDir, "secret.txt");
  writeFileSync(secretPath, "SUPER_SECRET_TOKEN=do-not-leak\n");

  const dir = mkdtempSync(join(tmpdir(), "lila-git-symlink-"));
  git(dir, ["init", "-b", "main"]);
  git(dir, ["config", "user.email", "test@example.com"]);
  git(dir, ["config", "user.name", "Test"]);
  writeFileSync(join(dir, "readme.md"), "hello\n");
  git(dir, ["add", "readme.md"]);
  git(dir, ["commit", "-m", "init"]);

  symlinkSync(secretPath, join(dir, "leak-link"));

  const diff = await gitDiff(dir, "leak-link");
  expect(diff.patch).toContain("+++ b/leak-link");
  expect(diff.patch).toContain(`+${secretPath}`);
  expect(diff.patch).not.toContain("SUPER_SECRET_TOKEN");
});

test("uses project-relative paths and preserves renames inside a repository subdirectory", async () => {
  const root = mkdtempSync(join(tmpdir(), "lila-git-subdir-"));
  const dir = join(root, "module");
  mkdirSync(dir);
  git(root, ["init", "-b", "main"]);
  git(root, ["config", "user.email", "test@example.com"]);
  git(root, ["config", "user.name", "Test"]);
  writeFileSync(join(dir, "old.ts"), "export const value = 1;\n");
  git(root, ["add", "."]);
  git(root, ["commit", "-m", "init"]);
  git(root, ["mv", "module/old.ts", "module/new.ts"]);
  writeFileSync(join(root, "outside.ts"), "outside");
  writeFileSync(join(dir, "untracked.ts"), "inside");

  const { files } = await gitStatus(dir);
  expect(files.map(({ path }) => path)).toEqual(["new.ts", "untracked.ts"]);
  expect(files[0]).toMatchObject({ path: "new.ts", from: "old.ts", status: "renamed" });
  const diff = await gitDiff(dir, "new.ts", true);
  expect(diff.patch).toContain("similarity index 100%");
  expect(diff.patch).toContain("rename from old.ts");
  expect(diff.patch).toContain("rename to new.ts");
  expect((await gitDiff(dir, "untracked.ts")).patch).toContain("+inside");
});

test("supports stage, unstage, discard, commit, and worktree creation within repository bounds", async () => {
  const parent = mkdtempSync(join(tmpdir(), "lila-git-ops-"));
  const dir = join(parent, "repo");
  mkdirSync(dir);
  git(dir, ["init", "-b", "main"]);
  git(dir, ["config", "user.email", "test@example.com"]);
  git(dir, ["config", "user.name", "Test"]);
  writeFileSync(join(dir, "a.ts"), "export const a = 1;\n");
  git(dir, ["add", "a.ts"]);
  git(dir, ["commit", "-m", "init"]);

  writeFileSync(join(dir, "a.ts"), "export const a = 2;\n");
  writeFileSync(join(dir, "temp.ts"), "export const tmp = 1;\n");

  await gitStage(dir, ["a.ts"]);
  expect((await gitStatus(dir)).files.find((f) => f.path === "a.ts")).toMatchObject({ staged: true, unstaged: false });

  await gitUnstage(dir, ["a.ts"]);
  expect((await gitStatus(dir)).files.find((f) => f.path === "a.ts")).toMatchObject({ staged: false, unstaged: true });

  await gitDiscard(dir, "a.ts");
  await gitDiscard(dir, "temp.ts");
  expect(readFileSync(join(dir, "a.ts"), "utf8")).toBe("export const a = 1;\n");
  expect(existsSync(join(dir, "temp.ts"))).toBe(false);
  await expect(gitDiscard(dir, "../escape.ts")).rejects.toThrow("不在项目目录内");

  writeFileSync(join(dir, "a.ts"), "export const a = 3;\n");
  await gitStage(dir, ["a.ts"]);
  const committed = await gitCommit(dir, "update a");
  expect(committed.hash.length).toBeGreaterThanOrEqual(7);
  expect((await gitStatus(dir)).files).toEqual([]);

  const wt = await gitCreateWorktree(dir, "feat/demo");
  expect(wt.branch).toBe("feat/demo");
  expect(existsSync(join(wt.path, "a.ts"))).toBe(true);
});

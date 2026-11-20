import { expect, test } from "bun:test";
import { execFileSync } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { gitDiff, gitStatus } from "./git.ts";

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

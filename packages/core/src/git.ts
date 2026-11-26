import { execFile } from "node:child_process";
import { randomBytes } from "node:crypto";
import { closeSync, existsSync, lstatSync, mkdirSync, openSync, readlinkSync, readSync, realpathSync, rmSync } from "node:fs";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { promisify } from "node:util";
import type { GitFileChange, GitFileDiff, GitFileStatus, GitStatus } from "@lila/protocol";
import { UserError } from "./errors.ts";

const execFileAsync = promisify(execFile);
const MAX_BUFFER = 16 * 1024 * 1024;
const MAX_PATCH_CHARS = 120_000;
const MAX_UNTRACKED_BYTES = 128 * 1024;

type GitExecResult = { stdout: string; stderr: string; code: number };

async function runGit(cwd: string, args: string[]): Promise<GitExecResult> {
  try {
    const { stdout, stderr } = await execFileAsync("git", ["-c", "core.quotepath=false", "--literal-pathspecs", ...args], { cwd, maxBuffer: MAX_BUFFER });
    return { stdout, stderr, code: 0 };
  } catch (error) {
    const err = error as NodeJS.ErrnoException & { stdout?: string; stderr?: string; code?: number | string };
    if (err.code === "ENOENT") throw new UserError("系统未安装 git");
    return {
      stdout: typeof err.stdout === "string" ? err.stdout : "",
      stderr: typeof err.stderr === "string" ? err.stderr : "",
      code: typeof err.code === "number" ? err.code : 1,
    };
  }
}

function mapStatusChar(x: string, y: string): GitFileStatus {
  const pair = `${x}${y}`;
  if (pair.includes("U") || pair === "AA" || pair === "DD") return "unmerged";
  if (x === "R" || y === "R") return "renamed";
  if (x === "C" || y === "C") return "copied";
  if (x === "A" || y === "A") return "added";
  if (x === "D" || y === "D") return "deleted";
  return "modified";
}

/** Splits the path field off a porcelain v2 line (`n` space-separated header fields before the path). */
function fieldAfter(record: string, spaces: number): string {
  let pos = 0;
  for (let i = 0; i < spaces; i++) {
    const next = record.indexOf(" ", pos);
    if (next < 0) return "";
    pos = next + 1;
  }
  return record.slice(pos);
}

/** Parses `git diff --numstat -z` into per-path added/removed totals. */
function parseNumstatZ(output: string): Map<string, { added: number; removed: number }> {
  const stats = new Map<string, { added: number; removed: number }>();
  if (!output) return stats;
  const parts = output.split("\0");
  for (let i = 0; i < parts.length; i++) {
    const part = parts[i];
    if (!part) continue;
    const firstTab = part.indexOf("\t");
    if (firstTab < 0) continue;
    const secondTab = part.indexOf("\t", firstTab + 1);
    if (secondTab < 0) continue;
    const addedRaw = part.slice(0, firstTab);
    const removedRaw = part.slice(firstTab + 1, secondTab);
    let path = part.slice(secondTab + 1);
    if (!path) {
      // Rename/copy format in `-z`: `<added>\t<removed>\t\0<orig>\0<new>\0`
      i += 2;
      path = parts[i] ?? "";
    }
    if (!path || addedRaw === "-" || removedRaw === "-") continue;
    const added = Number(addedRaw);
    const removed = Number(removedRaw);
    if (!Number.isFinite(added) || !Number.isFinite(removed)) continue;
    const existing = stats.get(path);
    stats.set(path, { added: (existing?.added ?? 0) + added, removed: (existing?.removed ?? 0) + removed });
  }
  return stats;
}

/** Porcelain v2 paths are repository-relative even when a pathspec limits the scan. */
async function readStatus(cwd: string, includeUntracked = true): Promise<GitStatus> {
  const [statusResult, prefixResult] = await Promise.all([
    runGit(cwd, ["status", "--porcelain=v2", "--branch", "-z", includeUntracked ? "-uall" : "-uno", "--", "."]),
    runGit(cwd, ["rev-parse", "--show-prefix"]),
  ]);
  if (statusResult.code !== 0) {
    if (/not a git repository/i.test(statusResult.stderr)) return { isRepo: false, files: [] };
    throw new UserError(`读取 Git 状态失败：${statusResult.stderr.trim() || "未知错误"}`);
  }

  if (prefixResult.code !== 0) throw new UserError("无法确定项目在 Git 仓库中的路径");
  const prefix = prefixResult.stdout.replace(/\r?\n$/, "");
  const toProjectPath = (path: string) => path.startsWith(prefix) ? path.slice(prefix.length) : "";

  let branch: string | undefined;
  let ahead: number | undefined;
  let behind: number | undefined;
  const files: GitFileChange[] = [];

  const records = statusResult.stdout.split("\0");
  for (let i = 0; i < records.length; i++) {
    const record = records[i];
    if (!record) continue;
    if (record.startsWith("# branch.head ")) {
      const value = record.slice("# branch.head ".length).trim();
      branch = value === "(detached)" ? "HEAD (detached)" : value;
    } else if (record.startsWith("# branch.ab ")) {
      const match = /\+(\d+)\s+-(\d+)/.exec(record);
      if (match) {
        ahead = Number(match[1]);
        behind = Number(match[2]);
      }
    } else if (record.startsWith("1 ")) {
      const xy = record.slice(2, 4);
      const path = toProjectPath(fieldAfter(record, 8));
      if (!path) continue;
      files.push({
        path,
        status: mapStatusChar(xy[0] ?? ".", xy[1] ?? "."),
        staged: xy[0] !== ".",
        unstaged: xy[1] !== ".",
      });
    } else if (record.startsWith("2 ")) {
      const xy = record.slice(2, 4);
      const path = toProjectPath(fieldAfter(record, 9));
      const from = toProjectPath(records[++i] ?? "");
      if (!path) continue;
      files.push({
        path,
        ...(from ? { from } : {}),
        status: mapStatusChar(xy[0] ?? ".", xy[1] ?? "."),
        staged: xy[0] !== ".",
        unstaged: xy[1] !== ".",
      });
    } else if (record.startsWith("u ")) {
      const path = toProjectPath(fieldAfter(record, 10));
      if (!path) continue;
      files.push({ path, status: "unmerged", staged: true, unstaged: true });
    } else if (record.startsWith("? ")) {
      const path = toProjectPath(record.slice(2));
      if (!path) continue;
      files.push({ path, status: "untracked", staged: false, unstaged: true });
    }
  }

  files.sort((a, b) => {
    if (a.status === "unmerged" && b.status !== "unmerged") return -1;
    if (b.status === "unmerged" && a.status !== "unmerged") return 1;
    return a.path.localeCompare(b.path);
  });

  return {
    isRepo: true,
    ...(branch ? { branch } : {}),
    ...(ahead !== undefined ? { ahead } : {}),
    ...(behind !== undefined ? { behind } : {}),
    files,
  };
}

/** Working-tree changes and line counts, all relative to the project directory. */
export async function gitStatus(cwd: string): Promise<GitStatus> {
  const status = await readStatus(cwd);
  const { files } = status;
  if (files.length > 0) {
    const [unstagedStats, stagedStats] = await Promise.all([
      runGit(cwd, ["diff", "--relative", "--numstat", "-z", "--", "."]).then((res) => (res.code === 0 ? parseNumstatZ(res.stdout) : new Map())),
      runGit(cwd, ["diff", "--cached", "--relative", "--numstat", "-z", "--", "."]).then((res) => (res.code === 0 ? parseNumstatZ(res.stdout) : new Map())),
    ]);

    for (const file of files) {
      const a = unstagedStats.get(file.path);
      const b = stagedStats.get(file.path);
      if (a || b) {
        file.added = (a?.added ?? 0) + (b?.added ?? 0);
        file.removed = (a?.removed ?? 0) + (b?.removed ?? 0);
      }
    }
  }

  return status;
}

function relativeOutside(path: string): boolean {
  return path === ".." || path.startsWith(`..${sep}`) || isAbsolute(path);
}

function assertInsideRepo(cwd: string, filePath: string): string {
  const root = resolve(cwd);
  const full = resolve(root, filePath);
  const rel = relative(root, full);
  if (!rel || relativeOutside(rel)) {
    throw new UserError(`文件路径不在项目目录内：${filePath}`);
  }
  return full;
}

function isWithinReal(root: string, target: string): boolean {
  const rel = relative(root, target);
  return !relativeOutside(rel);
}

function synthesizeUntrackedPatch(cwd: string, filePath: string, fullPath: string): GitFileDiff {
  let stat;
  try {
    stat = lstatSync(fullPath);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return { path: filePath, patch: "" };
    throw error;
  }

  const repoRealRoot = realpathSync(resolve(cwd));
  const parentReal = realpathSync(dirname(fullPath));
  if (!isWithinReal(repoRealRoot, parentReal)) {
    throw new UserError(`文件路径不在项目目录内：${filePath}`);
  }

  if (stat.isSymbolicLink()) {
    const target = readlinkSync(fullPath);
    const patch = `--- /dev/null\n+++ b/${filePath}\n@@ -0,0 +1,1 @@\n+${target}`;
    return clipPatch(filePath, patch);
  }

  if (!stat.isFile()) return { path: filePath, patch: "" };
  const fileReal = realpathSync(fullPath);
  if (!isWithinReal(repoRealRoot, fileReal)) {
    throw new UserError(`文件路径不在项目目录内：${filePath}`);
  }

  const bytesToRead = Math.min(stat.size, MAX_UNTRACKED_BYTES);
  const buffer = Buffer.alloc(bytesToRead);
  const fd = openSync(fileReal, "r");
  try {
    readSync(fd, buffer, 0, bytesToRead, 0);
  } finally {
    closeSync(fd);
  }
  const sampleLen = Math.min(bytesToRead, 8192);
  for (let i = 0; i < sampleLen; i++) {
    if (buffer[i] === 0) return { path: filePath, patch: "", binary: true };
  }
  const text = buffer.toString("utf8");
  const lines = text === "" ? [] : text.replace(/\n$/, "").split("\n");
  const body = lines.map((line) => `+${line}`).join("\n");
  const patch = `--- /dev/null\n+++ b/${filePath}\n@@ -0,0 +1,${lines.length} @@\n${body}`;
  return clipPatch(filePath, patch, stat.size > MAX_UNTRACKED_BYTES);
}

function clipPatch(filePath: string, patch: string, alreadyTruncated = false): GitFileDiff {
  if (/^Binary files .* differ$/m.test(patch) || /^GIT binary patch$/m.test(patch)) {
    return { path: filePath, patch: "", binary: true };
  }
  if (patch.length <= MAX_PATCH_CHARS) {
    return { path: filePath, patch, ...(alreadyTruncated ? { truncated: true } : {}) };
  }
  const cut = patch.lastIndexOf("\n", MAX_PATCH_CHARS);
  return { path: filePath, patch: patch.slice(0, cut > 0 ? cut : MAX_PATCH_CHARS), truncated: true };
}

/** Unified diff for one file in a project repository. */
export async function gitDiff(cwd: string, filePath: string, staged?: boolean): Promise<GitFileDiff> {
  const fullPath = assertInsideRepo(cwd, filePath);
  const path = relative(resolve(cwd), fullPath).split(sep).join("/");

  if (staged !== true) {
    const untrackedCheck = await runGit(cwd, ["ls-files", "--others", "--exclude-standard", "--", path]);
    if (untrackedCheck.code === 0 && untrackedCheck.stdout.length > 0) {
      return synthesizeUntrackedPatch(cwd, filePath, fullPath);
    }
  }

  // The old path must be part of the pathspec for Git to emit rename/copy metadata.
  const diff = ["diff", "--relative"];
  const paths = [path];
  if (staged !== false) {
    const status = await readStatus(cwd, false);
    const change = status.files.find((file) => file.path === path);
    if (change?.from) {
      paths.push(change.from);
      diff.push(change.status === "copied" ? "--find-copies" : "--find-renames");
    }
  }

  if (staged !== undefined) {
    const result = await runGit(cwd, [...diff, ...(staged ? ["--cached"] : []), "--", ...paths]);
    return clipPatch(filePath, result.stdout);
  }

  const combined = await runGit(cwd, [...diff, "HEAD", "--", ...paths]);
  if (combined.code === 0) return clipPatch(filePath, combined.stdout);

  // Unborn HEAD (brand-new repo with no commits yet): fall back to cached + unstaged.
  const [cached, work] = await Promise.all([
    runGit(cwd, [...diff, "--cached", "--", ...paths]),
    runGit(cwd, [...diff, "--", ...paths]),
  ]);
  return clipPatch(filePath, [cached.stdout.trimEnd(), work.stdout.trimEnd()].filter(Boolean).join("\n"));
}

function toRelPaths(cwd: string, paths: readonly string[]): string[] {
  const root = resolve(cwd);
  return paths.map((filePath) => {
    const full = assertInsideRepo(cwd, filePath);
    return relative(root, full).split(sep).join("/");
  });
}

/** Stages specific files or the entire working tree within the project directory. */
export async function gitStage(cwd: string, paths?: readonly string[]): Promise<GitStatus> {
  const args = paths?.length ? ["add", "--", ...toRelPaths(cwd, paths)] : ["add", "-A", "--", "."];
  const res = await runGit(cwd, args);
  if (res.code !== 0) throw new UserError(`暂存失败：${res.stderr.trim() || "未知错误"}`);
  return gitStatus(cwd);
}

/** Unstages specific files or all staged changes within the project directory. */
export async function gitUnstage(cwd: string, paths?: readonly string[]): Promise<GitStatus> {
  const rels = paths?.length ? toRelPaths(cwd, paths) : ["."];
  const hasHead = (await runGit(cwd, ["rev-parse", "--verify", "HEAD"])).code === 0;
  const args = hasHead ? ["reset", "HEAD", "--", ...rels] : ["rm", "--cached", "-r", "--quiet", "--", ...rels];
  const res = await runGit(cwd, args);
  if (res.code !== 0) throw new UserError(`取消暂存失败：${res.stderr.trim() || "未知错误"}`);
  return gitStatus(cwd);
}

/** Discards working-tree changes for one file, or removes an untracked file safely inside the project. */
export async function gitDiscard(cwd: string, filePath: string): Promise<GitStatus> {
  const fullPath = assertInsideRepo(cwd, filePath);
  const relPath = relative(resolve(cwd), fullPath).split(sep).join("/");

  const untracked = await runGit(cwd, ["ls-files", "--others", "--exclude-standard", "--", relPath]);
  if (untracked.code === 0 && untracked.stdout.length > 0) {
    const repoRealRoot = realpathSync(resolve(cwd));
    const parentReal = realpathSync(dirname(fullPath));
    if (!isWithinReal(repoRealRoot, parentReal)) {
      throw new UserError(`文件路径不在项目目录内：${filePath}`);
    }
    rmSync(fullPath, { recursive: true, force: true });
    return gitStatus(cwd);
  }

  const res = await runGit(cwd, ["checkout", "--", relPath]);
  if (res.code !== 0) {
    // Newly staged file (added, not in HEAD yet): restore both index and worktree or remove from index.
    const hasHead = (await runGit(cwd, ["rev-parse", "--verify", "HEAD"])).code === 0;
    const fallback = hasHead
      ? await runGit(cwd, ["checkout", "HEAD", "--", relPath])
      : await runGit(cwd, ["rm", "--cached", "-f", "--", relPath]);
    if (fallback.code !== 0) {
      throw new UserError(`丢弃改动失败：${res.stderr.trim() || fallback.stderr.trim() || "未知错误"}`);
    }
  }
  return gitStatus(cwd);
}

/** Creates a Git commit from currently staged changes. */
export async function gitCommit(cwd: string, message: string): Promise<{ status: GitStatus; hash: string }> {
  const trimmed = message.trim();
  if (!trimmed) throw new UserError("提交说明不能为空");
  const res = await runGit(cwd, ["commit", "-m", trimmed]);
  if (res.code !== 0) {
    throw new UserError(`提交失败：${res.stderr.trim() || res.stdout.trim() || "请先暂存要提交的文件"}`);
  }
  const rev = await runGit(cwd, ["rev-parse", "--short", "HEAD"]);
  const status = await gitStatus(cwd);
  return { status, hash: rev.stdout.trim() };
}

/** Creates a sibling Git worktree on a new (or existing) branch so parallel agents work in isolation. */
export async function gitCreateWorktree(cwd: string, requestedBranch?: string): Promise<{ path: string; branch: string }> {
  const top = await runGit(cwd, ["rev-parse", "--show-toplevel"]);
  if (top.code !== 0 || !top.stdout.trim()) throw new UserError("当前项目不是 Git 仓库，无法创建 Worktree");
  const repoRoot = top.stdout.trim();
  const repoName = basename(repoRoot) || "repo";
  const branch =
    requestedBranch?.trim() ||
    `lila/${new Date().toISOString().slice(5, 10).replace("-", "")}-${randomBytes(2).toString("hex")}`;
  if (branch.startsWith("-") || /\s|\.\.|[~^:?*[\]\\]/.test(branch)) {
    throw new UserError(`分支名不合法：${branch}`);
  }
  const safeDirName = branch.replace(/[^A-Za-z0-9._-]+/g, "-").replace(/^-+|-+$/g, "") || "worktree";
  const worktreesBase = join(dirname(repoRoot), `${repoName}-worktrees`);
  mkdirSync(worktreesBase, { recursive: true });
  let targetDir = join(worktreesBase, safeDirName);
  if (existsSync(targetDir)) {
    targetDir = join(worktreesBase, `${safeDirName}-${randomBytes(2).toString("hex")}`);
  }
  const branchExists = (await runGit(repoRoot, ["show-ref", "--verify", "--quiet", `refs/heads/${branch}`])).code === 0;
  const addArgs = branchExists
    ? ["worktree", "add", targetDir, branch]
    : ["worktree", "add", "-b", branch, targetDir];
  const res = await runGit(repoRoot, addArgs);
  if (res.code !== 0) throw new UserError(`创建 Worktree 失败：${res.stderr.trim() || res.stdout.trim() || "未知错误"}`);
  return { path: targetDir, branch };
}

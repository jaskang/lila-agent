import { execFile } from "node:child_process";
import { closeSync, lstatSync, openSync, readlinkSync, readSync, realpathSync } from "node:fs";
import { dirname, isAbsolute, relative, resolve, sep } from "node:path";
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
    const { stdout, stderr } = await execFileAsync("git", ["--literal-pathspecs", ...args], { cwd, maxBuffer: MAX_BUFFER });
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

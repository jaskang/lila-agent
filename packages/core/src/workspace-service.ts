import { execFile } from "node:child_process";
import { existsSync, realpathSync, statSync } from "node:fs";
import { open, readdir } from "node:fs/promises";
import { extname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { promisify } from "node:util";
import { getAgentDir } from "@earendil-works/pi-coding-agent";
import type { ProjectFileContent } from "@lila/protocol";
import fuzzysort from "fuzzysort";
import { UserError } from "./errors.ts";

const WALK_LIMIT = 20_000;
const TREE_FILE_LIMIT = 3_000;
const FILE_PREVIEW_MAX_BYTES = 512 * 1024;
const IMAGE_EXTS = new Set([".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg", ".avif", ".bmp", ".ico"]);
const WALK_SKIP: Record<string, true> = { node_modules: true, ".git": true };
const FILE_CACHE_TTL_MS = 8_000;
const MAX_FILE_CACHE_ENTRIES = 32;
const execFileAsync = promisify(execFile);
const fileListCache = new Map<string, { files: string[]; expiresAt: number }>();

export function invalidateFileCache(projectPath: string): void {
  fileListCache.delete(projectPath);
}

function resolveRealOrAbs(target: string): string {
  try {
    return realpathSync(target);
  } catch {
    return resolve(target);
  }
}

function isWithinDir(root: string, target: string): boolean {
  const rel = relative(root, target);
  return rel === "" || (!rel.startsWith("..") && !isAbsolute(rel));
}

export async function readPreviewFile(projectRoot: string, rawPath: string): Promise<ProjectFileContent> {
  const candidate = isAbsolute(rawPath) ? resolve(rawPath) : resolve(projectRoot, rawPath);
  if (!existsSync(candidate)) throw new UserError(`找不到文件：${rawPath}`);

  let realTarget: string;
  try {
    realTarget = realpathSync(candidate);
  } catch {
    throw new UserError(`无法访问文件：${rawPath}`);
  }

  const realProject = resolveRealOrAbs(projectRoot);
  const realAgent = resolveRealOrAbs(getAgentDir());
  const inProject = isWithinDir(realProject, realTarget);
  const inAgent = isWithinDir(realAgent, realTarget);
  if (!inProject && !inAgent) {
    throw new UserError(`不允许读取项目目录之外的文件：${rawPath}`);
  }

  const info = statSync(realTarget);
  if (!info.isFile()) throw new UserError(`只能预览文件，无法打开目录：${rawPath}`);

  const displayPath = inProject ? relative(realProject, realTarget).split(sep).join("/") : realTarget;
  const ext = extname(realTarget).toLowerCase();
  if (IMAGE_EXTS.has(ext)) {
    return {
      path: displayPath,
      fullPath: realTarget,
      size: info.size,
      isBinary: true,
      isImage: true,
      truncated: false,
      content: "",
    };
  }

  const handle = await open(realTarget, "r");
  try {
    const readSize = Math.min(info.size, FILE_PREVIEW_MAX_BYTES);
    const buffer = Buffer.allocUnsafe(readSize);
    const { bytesRead } = await handle.read(buffer, 0, readSize, 0);
    const slice = buffer.subarray(0, bytesRead);
    const probe = slice.subarray(0, Math.min(slice.length, 8_192));
    const isBinary = probe.includes(0);
    return {
      path: displayPath,
      fullPath: realTarget,
      size: info.size,
      isBinary,
      isImage: false,
      truncated: info.size > FILE_PREVIEW_MAX_BYTES,
      content: isBinary ? "" : slice.toString("utf8"),
    };
  } finally {
    await handle.close();
  }
}

/** Project files relative to its root: git's view when it is a repository, otherwise a bounded walk. */
export async function listFiles(root: string, refresh = false): Promise<string[]> {
  const now = Date.now();
  const cached = !refresh ? fileListCache.get(root) : undefined;
  if (cached && now < cached.expiresAt) return cached.files;

  let files: string[];
  try {
    const { stdout } = await execFileAsync(
      "git",
      ["-c", "core.quotepath=false", "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
      { cwd: root, maxBuffer: 64 * 1024 * 1024 },
    );
    files = stdout.split("\0").filter(Boolean);
  } catch {
    files = [];
    const queue = [root];
    while (queue.length > 0 && files.length < WALK_LIMIT) {
      const dir = queue.shift() as string;
      const entries = await readdir(dir, { withFileTypes: true }).catch(() => []);
      for (const entry of entries) {
        if (WALK_SKIP[entry.name]) continue;
        const path = join(dir, entry.name);
        if (entry.isDirectory()) queue.push(path);
        else if (entry.isFile()) files.push(relative(root, path));
        if (files.length >= WALK_LIMIT) break;
      }
    }
  }

  if (fileListCache.size >= MAX_FILE_CACHE_ENTRIES) {
    for (const [key, entry] of fileListCache) {
      if (now >= entry.expiresAt) fileListCache.delete(key);
    }
    if (fileListCache.size >= MAX_FILE_CACHE_ENTRIES) {
      const oldest = fileListCache.keys().next().value;
      if (oldest !== undefined) fileListCache.delete(oldest);
    }
  }
  fileListCache.set(root, { files, expiresAt: now + FILE_CACHE_TTL_MS });
  return files;
}

export async function searchProjectFiles(root: string, query: string, limit: number): Promise<string[]> {
  const files = await listFiles(root);
  if (!query.trim()) return files.slice(0, limit);
  return fuzzysort.go(query.trim(), files, { limit }).map((result) => result.target);
}

export async function listTreeFiles(root: string, refresh = false): Promise<{ files: string[]; truncated: boolean }> {
  const files = await listFiles(root, refresh);
  return {
    files: files.slice(0, TREE_FILE_LIMIT),
    truncated: files.length > TREE_FILE_LIMIT,
  };
}

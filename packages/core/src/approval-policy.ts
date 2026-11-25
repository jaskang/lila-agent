import { lstatSync, realpathSync } from "node:fs";
import { homedir } from "node:os";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import type { ToolInfo } from "@earendil-works/pi-coding-agent";
import type { ApprovalMode } from "@lila/protocol";

const LOCAL_READS = new Set(["read", "grep", "find", "ls"]);
const MCP_DISCOVERY = new Set(["list_mcp_resources", "list_mcp_resource_templates"]);
const NONSTANDARD_PATH = /[\u00a0\u2000-\u200a\u202f\u205f\u3000]/u;
type ApprovalTool = Pick<ToolInfo, "sourceInfo" | "annotations">;

function canonicalWriteTarget(path: string): string | undefined {
  try {
    return realpathSync(path);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") return;
  }

  const parent = dirname(path);
  if (parent === path) return;
  try {
    // If the path exists but could not be resolved (such as a dangling symlink), ask.
    lstatSync(path);
    return;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") return;
  }
  const canonicalParent = canonicalWriteTarget(parent);
  return canonicalParent ? join(canonicalParent, basename(path)) : undefined;
}

function describeFileWrite(rawPath: unknown, cwd: string, projectRoot: string): string | undefined {
  if (typeof rawPath !== "string" || !rawPath || rawPath.includes("\0") || NONSTANDARD_PATH.test(rawPath)) return "无法确认文件写入位置";
  const path = rawPath.startsWith("@") ? rawPath.slice(1) : rawPath;
  if (/^file:/i.test(path) || (process.platform === "win32" && path.startsWith("/"))) return "无法确认文件写入位置";
  const homeRelative = /^~(?:[\\/]|$)/.test(path);
  const target = homeRelative ? resolve(homedir(), path.length > 1 ? path.slice(2) : "") : resolve(cwd, path);
  const canonicalTarget = canonicalWriteTarget(target);
  let canonicalProject: string;
  try {
    canonicalProject = realpathSync(projectRoot);
  } catch {
    return "无法确认项目目录";
  }
  if (!canonicalTarget) return "无法确认文件写入位置";
  const fromProject = relative(canonicalProject, canonicalTarget);
  if (fromProject === ".." || fromProject.startsWith(`..${sep}`) || isAbsolute(fromProject)) {
    return `写入项目目录外的文件：${canonicalTarget}`;
  }
  return undefined;
}

/** Classify observable tool inputs. A granted tool still runs with its usual process permissions. */
export function approvalReason({
  mode, projectRoot, cwd, name, input, tool,
}: {
  mode: ApprovalMode;
  projectRoot: string;
  cwd: string;
  name: string;
  input: Record<string, unknown>;
  tool: ApprovalTool | undefined;
}): string | undefined {
  if (mode === "auto") return;
  const source = tool?.sourceInfo.path;

  if (source === `builtin:${name}`) {
    if (LOCAL_READS.has(name)) return;
    if (name === "write" || name === "edit") return describeFileWrite(input.path, cwd, projectRoot);
    if (name === "bash" || name === "powershell") {
      if (name === "bash" && typeof input.command === "string" && ["pwd", "true", ":"].includes(input.command.trim())) return;
      return "命令可能写入项目外或联网，执行前无法确认影响范围";
    }
  }

  if (source === "builtin:lila" && name === "ask_user") return;
  if (source === "builtin:tool-search" && name === "tool_search") return;
  if (name === "memory_read" && tool?.annotations?.readOnlyHint === true) return;
  if (name === "get_subagent_result" && tool?.annotations?.readOnlyHint === true) return;
  if (name === "Agent") return "子 Agent 会在独立执行环境中运行工具";
  if (name === "codemode") return "脚本可调用其他工具或模型服务";
  if (name === "memory_add" || name === "memory_replace" || name === "memory_remove") return "修改项目外的长期记忆";

  if (source === "builtin:mcp") {
    if (mode === "ask-exec" && (MCP_DISCOVERY.has(name) || (
      tool?.annotations?.readOnlyHint === true && tool.annotations.openWorldHint === false
    ))) return;
    return "MCP 调用可能连接服务器或影响项目外资源";
  }
  return "扩展工具的项目外访问和联网行为无法确认";
}

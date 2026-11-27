import { lstatSync, realpathSync } from "node:fs";
import { homedir } from "node:os";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import type { ToolInfo } from "@earendil-works/pi-coding-agent";
import type { ApprovalMode, ApprovalRuleSuggestion } from "@lila/protocol";

const LOCAL_READS = new Set(["read", "grep", "find", "ls"]);
const MCP_DISCOVERY = new Set(["list_mcp_resources", "list_mcp_resource_templates"]);
const MEMORY_WRITES = new Set(["memory_add", "memory_replace", "memory_remove"]);
const LILA_READS = new Set(["ask_user", "schedule_list", "task_list"]);
const LILA_WRITES = new Set(["schedule_create", "schedule_update", "schedule_delete", "task_upsert"]);
const NONSTANDARD_PATH = /[\u00a0\u2000-\u200a\u202f\u205f\u3000]/u;

/** Characters that enable redirection, command substitution, backgrounding, or multi-statement chaining. */
const UNSAFE_SHELL_SYNTAX = /[\r\n`><;()&{}]|(\$\()/;

/** Commands that only inspect state when invoked without shell redirections. */
const READONLY_COMMANDS = new Set([
  "pwd",
  "true",
  ":",
  "ls",
  "ll",
  "la",
  "cat",
  "head",
  "tail",
  "wc",
  "which",
  "type",
  "dirname",
  "basename",
  "realpath",
  "readlink",
  "stat",
  "file",
  "du",
  "df",
  "uname",
  "whoami",
  "id",
  "date",
  "echo",
  "printf",
  "rg",
  "grep",
  "egrep",
  "fgrep",
  "fd",
  "tree",
  "sort",
  "uniq",
  "cut",
  "tr",
  "diff",
  "jq",
  "ps",
]);

const FIND_MUTATING_FLAGS = new Set(["-exec", "-execdir", "-ok", "-okdir", "-delete", "-fls", "-fprint", "-fprint0", "-fprintf"]);
const GIT_READONLY_SUBCOMMANDS = new Set(["status", "diff", "log", "show", "rev-parse", "ls-files", "ls-tree", "describe", "shortlog", "blame", "grep"]);
const GIT_BRANCH_READONLY_FLAGS = new Set(["-a", "--all", "-r", "--remotes", "-v", "-vv", "--verbose", "--list", "--show-current", "--merged", "--no-merged"]);
const VERSION_ONLY_COMMANDS = new Set(["node", "npm", "pnpm", "yarn", "bun", "python", "python3", "go", "rustc", "cargo", "tsc", "vitest", "gh"]);
const TWO_TOKEN_RULE_PREFIXES = new Set([
  "npm",
  "pnpm",
  "yarn",
  "bun",
  "npx",
  "git",
  "cargo",
  "docker",
  "kubectl",
  "uv",
  "pip",
  "pip3",
  "python",
  "python3",
  "go",
  "gh",
  "brew",
]);
const BASH_RULE_FORBIDDEN_HEADS = new Set(["sh", "bash", "zsh", "fish", "dash", "eval", "exec", "sudo", "su", "rm", "dd", "mkfs", "chmod", "chown"]);

type ApprovalTool = Pick<ToolInfo, "sourceInfo" | "annotations">;

/** Splits a single simple command into shell-like tokens; returns undefined on unclosed quotes or escapes. */
function tokenizeSimpleCommand(segment: string): string[] | undefined {
  const tokens: string[] = [];
  let current = "";
  let quote: "'" | '"' | undefined;
  for (let i = 0; i < segment.length; i++) {
    const ch = segment[i]!;
    if (ch === "\\" && quote !== "'") return undefined;
    if (quote) {
      if (ch === quote) quote = undefined;
      else current += ch;
      continue;
    }
    if (ch === "'" || ch === '"') {
      quote = ch;
      continue;
    }
    if (/\s/.test(ch)) {
      if (current) {
        tokens.push(current);
        current = "";
      }
      continue;
    }
    current += ch;
  }
  if (quote) return undefined;
  if (current) tokens.push(current);
  return tokens;
}

/** Splits a command by single pipes `|` outside quotes; rejects `||` or unsafe shell syntax. */
function splitReadonlyPipeline(command: string): string[][] | undefined {
  const trimmed = command.trim();
  if (!trimmed || UNSAFE_SHELL_SYNTAX.test(trimmed)) return undefined;
  const segments: string[] = [];
  let current = "";
  let quote: "'" | '"' | undefined;
  for (let i = 0; i < trimmed.length; i++) {
    const ch = trimmed[i]!;
    if (ch === "\\" && quote !== "'") return undefined;
    if (quote) {
      if (ch === quote) quote = undefined;
      current += ch;
      continue;
    }
    if (ch === "'" || ch === '"') {
      quote = ch;
      current += ch;
      continue;
    }
    if (ch === "|") {
      if (trimmed[i + 1] === "|") return undefined;
      segments.push(current);
      current = "";
      continue;
    }
    current += ch;
  }
  if (quote) return undefined;
  segments.push(current);
  const tokenized: string[][] = [];
  for (const seg of segments) {
    const tokens = tokenizeSimpleCommand(seg.trim());
    if (!tokens || !tokens.length) return undefined;
    tokenized.push(tokens);
  }
  return tokenized;
}

function isReadonlyGit(args: readonly string[]): boolean {
  let index = 0;
  while (index < args.length && args[index]!.startsWith("-")) {
    const flag = args[index]!;
    if (flag === "--no-pager" || flag === "--paginate") {
      index++;
      continue;
    }
    // Reject `-c`, `--git-dir`, `--work-tree`, etc. which can execute arbitrary hooks/commands.
    return false;
  }
  const sub = args[index];
  if (!sub) return false;
  const rest = args.slice(index + 1);
  if (GIT_READONLY_SUBCOMMANDS.has(sub)) return !rest.some((arg) => arg === "--ext-diff" || arg.startsWith("--output="));
  if (sub === "branch") return rest.every((arg) => GIT_BRANCH_READONLY_FLAGS.has(arg));
  if (sub === "tag") return rest.length === 0 || (rest.length === 1 && (rest[0] === "-l" || rest[0] === "--list"));
  if (sub === "remote") return rest.length === 0 || (rest.length === 1 && (rest[0] === "-v" || rest[0] === "--verbose"));
  if (sub === "stash") return rest.length === 1 && rest[0] === "list";
  if (sub === "worktree") return rest.length === 1 && rest[0] === "list";
  return false;
}

function isReadonlySegment(tokens: readonly string[]): boolean {
  const [head, ...args] = tokens;
  if (!head) return false;
  if (READONLY_COMMANDS.has(head)) return true;
  if (head === "find") return !args.some((arg) => FIND_MUTATING_FLAGS.has(arg));
  if (head === "git") return isReadonlyGit(args);
  if (VERSION_ONLY_COMMANDS.has(head)) {
    return args.length === 1 && (args[0] === "-v" || args[0] === "-V" || args[0] === "--version");
  }
  return false;
}

/** True when a shell command is statically verifiable as read-only without redirections or subshells. */
export function isSafeReadonlyBash(command: string): boolean {
  const pipeline = splitReadonlyPipeline(command);
  if (!pipeline) return false;
  return pipeline.every(isReadonlySegment);
}

/** Derives a reusable approval rule suggestion for a tool invocation, or undefined when per-call review is safer. */
export function suggestApprovalRule(
  name: string,
  input: Record<string, unknown>,
  tool: ApprovalTool | undefined,
): ApprovalRuleSuggestion | undefined {
  const source = tool?.sourceInfo.path;
  if (source === `builtin:${name}`) {
    if (name === "bash" || name === "powershell") {
      if (typeof input.command !== "string") return undefined;
      const trimmed = input.command.trim();
      if (!trimmed || UNSAFE_SHELL_SYNTAX.test(trimmed) || trimmed.includes("|")) return undefined;
      const tokens = tokenizeSimpleCommand(trimmed);
      if (!tokens || !tokens.length) return undefined;
      const head = tokens[0]!;
      if (BASH_RULE_FORBIDDEN_HEADS.has(head) || head.includes("/") || head.includes("=")) return undefined;
      if (TWO_TOKEN_RULE_PREFIXES.has(head) && tokens.length >= 2 && !tokens[1]!.startsWith("-")) {
        const sub = tokens[1]!;
        if (sub.includes("/") || sub.includes("=")) return undefined;
        const prefix = `${head} ${sub}`;
        return { pattern: `bash:${prefix}`, label: `${prefix} *` };
      }
      return { pattern: `bash:${head}`, label: `${head} *` };
    }
    return undefined;
  }

  if (source === "builtin:mcp") {
    const mcpLabel = name.startsWith("mcp__") ? name.slice(5).replace("__", " / ") : name;
    return { pattern: `mcp:${name}`, label: `MCP ${mcpLabel}` };
  }

  return { pattern: `tool:${name}`, label: `工具 ${name}` };
}

/** True when any granted rule covers this tool invocation safely. */
export function matchesApprovalRule(
  rules: Iterable<string>,
  name: string,
  input: Record<string, unknown>,
  tool: ApprovalTool | undefined,
): boolean {
  const suggestion = suggestApprovalRule(name, input, tool);
  if (!suggestion) return false;
  for (const rule of rules) {
    if (rule === suggestion.pattern) return true;
  }
  return false;
}

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
  mode, projectRoot, cwd, name, input, tool, rules,
}: {
  mode: ApprovalMode;
  projectRoot: string;
  cwd: string;
  name: string;
  input: Record<string, unknown>;
  tool: ApprovalTool | undefined;
  rules?: Iterable<string>;
}): string | undefined {
  if (mode === "auto") return;
  if (rules && matchesApprovalRule(rules, name, input, tool)) return;
  const source = tool?.sourceInfo.path;

  if (source === `builtin:${name}`) {
    if (LOCAL_READS.has(name)) return;
    if (name === "write" || name === "edit") return describeFileWrite(input.path, cwd, projectRoot);
    if (name === "bash" || name === "powershell") {
      if (name === "bash" && typeof input.command === "string") {
        if (["pwd", "true", ":"].includes(input.command.trim())) return;
        if (mode === "ask-exec" && isSafeReadonlyBash(input.command)) return;
      }
      return "命令可能写入项目外或联网，执行前无法确认影响范围";
    }
  }

  if (source === "builtin:lila" && LILA_READS.has(name)) return;
  if (source === "builtin:lila" && LILA_WRITES.has(name)) {
    if (mode === "ask-exec") return;
    return "修改后台定时任务或任务收件箱";
  }
  if (source === "builtin:tool-search" && name === "tool_search") return;
  if (name === "memory_read" && tool?.annotations?.readOnlyHint === true) return;
  if (name === "get_subagent_result" && tool?.annotations?.readOnlyHint === true) return;
  if (name === "Agent") return "子 Agent 会在独立执行环境中运行工具";
  if (name === "codemode") return "脚本可调用其他工具或模型服务";
  if (MEMORY_WRITES.has(name)) {
    if (mode === "ask-exec") return;
    return "修改项目外的长期记忆";
  }

  if (source === "builtin:mcp") {
    if (mode === "ask-exec" && (MCP_DISCOVERY.has(name) || (
      tool?.annotations?.readOnlyHint === true && tool.annotations.openWorldHint === false
    ))) return;
    return "MCP 调用可能连接服务器或影响项目外资源";
  }
  return "扩展工具的项目外访问和联网行为无法确认";
}

import { randomUUID } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type {
  AssistantTask,
  SessionStatus,
  TaskCreateInput,
  TaskLink,
  TaskLinkKind,
  TaskPriority,
  TaskStatus,
  TaskUpdatePatch,
} from "@lila/protocol";
import { UserError } from "./errors.ts";

const STATUS_ORDER: Record<TaskStatus, number> = {
  review: 0,
  clarifying: 1,
  running: 2,
  inbox: 3,
  done: 4,
};

const PRIORITY_ORDER: Record<TaskPriority, number> = {
  high: 0,
  normal: 1,
  low: 2,
};

const URL_PATTERN = /https?:\/\/[^\s<>"')\]}]+/gi;

/** Infers link kind (design / issue / pr / doc / other) from URL patterns. */
export function inferTaskLinkKind(url: string): TaskLinkKind {
  try {
    const parsed = new URL(url);
    const host = parsed.hostname.toLowerCase();
    const path = parsed.pathname.toLowerCase();
    if (host === "github.com" || host.endsWith(".github.com")) {
      if (path.includes("/pull/")) return "pr";
      if (path.includes("/issues/")) return "issue";
    }
    if (host.includes("asana.com") || host.includes("linear.app") || host.includes("jira")) return "issue";
    if (
      host.includes("figma.com") ||
      host.includes("lanhuapp") ||
      host.includes("mastergo") ||
      host.includes("js.design") ||
      host.includes("modao") ||
      host.endsWith(".vercel.app") ||
      host.endsWith(".netlify.app") ||
      host.endsWith(".pages.dev") ||
      path.endsWith(".html") ||
      path.endsWith(".htm") ||
      /\/(design|prototype|mockup|preview)\b/.test(path)
    ) {
      return "design";
    }
    if (host.includes("notion.so") || host.includes("feishu.cn") || host.includes("docs.google.com") || host.includes("yuque.com")) {
      return "doc";
    }
  } catch {
    // Keep fallback kind for malformed URLs.
  }
  return "other";
}

/** Extracts deduplicated URLs from free-form pasted chat messages and merges them with explicit links. */
export function mergeTaskLinks(explicit: readonly TaskLink[] | undefined, summary: string): TaskLink[] {
  const seen = new Set<string>();
  const result: TaskLink[] = [];
  const push = (rawUrl: string, label?: string, kind?: TaskLinkKind) => {
    const cleaned = rawUrl.trim().replace(/[.,;:!?。，；：！？]+$/, "");
    if (!cleaned || seen.has(cleaned)) return;
    seen.add(cleaned);
    result.push({
      url: cleaned,
      ...(label?.trim() ? { label: label.trim() } : {}),
      kind: kind ?? inferTaskLinkKind(cleaned),
    });
  };
  for (const link of explicit ?? []) push(link.url, link.label, link.kind);
  for (const match of summary.match(URL_PATTERN) ?? []) push(match);
  return result.slice(0, 30);
}

/** Builds a structured execution prompt when kicking off an AssistantTask into a session. */
export function buildTaskExecutionPrompt(task: AssistantTask): string {
  const lines: string[] = [
    `请执行以下开发任务（任务 ID: ${task.id}）：`,
    "",
    `# ${task.title}`,
  ];
  if (task.summary.trim()) {
    lines.push("", "## 需求说明与上下文", task.summary.trim());
  }
  if (task.links.length) {
    lines.push("", "## 相关链接");
    for (const link of task.links) {
      const tag =
        link.kind === "design"
          ? "[设计稿]"
          : link.kind === "issue"
            ? "[需求/Issue]"
            : link.kind === "pr"
              ? "[PR]"
              : link.kind === "doc"
                ? "[文档]"
                : "[链接]";
      lines.push(`- ${tag}${link.label ? ` ${link.label}：` : " "}${link.url}`);
    }
  }
  if (task.blockingQuestions.length) {
    lines.push("", "## 已知待澄清项");
    for (const q of task.blockingQuestions) lines.push(`- ${q}`);
  }
  lines.push("", "## 执行要求");
  if (task.links.some((link) => link.kind === "design")) {
    lines.push("- 包含网页设计稿链接：如可用浏览器工具，请先完整浏览设计稿页面及其可点击交互/多页面跳转，严格映射到当前仓库的组件与样式体系实现，不要直接拷贝原型 HTML。");
  }
  lines.push(
    "- 先核对需求与代码现状：如果存在关键阻塞信息缺失，请使用 `task_upsert` 将任务状态更新为 `clarifying`、写入 `blockingQuestions` 并起草对外确认话术 `draftReply`。",
    "- 如果需求明确，请直接实施代码修改并运行项目的类型检查/测试/构建进行验证。",
    "- 完成后，请使用 `task_upsert` 更新该任务状态为 `review`，并在 `draftReply` 中起草一段简洁、可直接发给相关干系人的进度/验收汇报草稿。",
  );
  return lines.join("\n");
}

type PersistedTasks = { version: 1; tasks: AssistantTask[] };

/** Persistent cross-platform task inbox & state machine backing Lila's personal assistant workflow. */
export class TaskService {
  readonly #path: string;
  readonly #onChange: (tasks: AssistantTask[]) => void;
  readonly #tasks = new Map<string, AssistantTask>();

  constructor(dataDir: string, onChange: (tasks: AssistantTask[]) => void) {
    mkdirSync(dataDir, { recursive: true, mode: 0o700 });
    this.#path = join(dataDir, "tasks.json");
    this.#onChange = onChange;
    this.#load();
  }

  list(): AssistantTask[] {
    return Array.from(this.#tasks.values(), (task) => ({
      ...task,
      links: task.links.map((link) => ({ ...link })),
      blockingQuestions: [...task.blockingQuestions],
    })).sort((a, b) => {
      const statusDiff = STATUS_ORDER[a.status] - STATUS_ORDER[b.status];
      if (statusDiff !== 0) return statusDiff;
      const priorityDiff = PRIORITY_ORDER[a.priority] - PRIORITY_ORDER[b.priority];
      if (priorityDiff !== 0) return priorityDiff;
      return Date.parse(b.updatedAt) - Date.parse(a.updatedAt);
    });
  }

  get(id: string): AssistantTask | undefined {
    return this.#tasks.get(id);
  }

  create(input: TaskCreateInput): AssistantTask {
    const now = new Date().toISOString();
    const summary = input.summary?.trim() ?? "";
    const blockingQuestions = (input.blockingQuestions ?? []).map((item) => item.trim()).filter(Boolean);
    const status: TaskStatus = input.status ?? (blockingQuestions.length > 0 ? "clarifying" : "inbox");
    const task: AssistantTask = {
      id: randomUUID(),
      title: input.title.trim(),
      ...(input.projectId ? { projectId: input.projectId } : {}),
      status,
      priority: input.priority ?? "normal",
      source: input.source ?? "manual",
      summary,
      links: mergeTaskLinks(input.links, summary),
      blockingQuestions,
      ...(input.draftReply?.trim() ? { draftReply: input.draftReply.trim() } : {}),
      createdAt: now,
      updatedAt: now,
    };
    this.#tasks.set(task.id, task);
    this.#saveAndBroadcast();
    return task;
  }

  update(id: string, patch: TaskUpdatePatch): AssistantTask {
    const existing = this.#tasks.get(id);
    if (!existing) throw new UserError("任务不存在");

    const nextSummary = patch.summary !== undefined ? patch.summary.trim() : existing.summary;
    const nextLinks =
      patch.links !== undefined
        ? mergeTaskLinks(patch.links, "")
        : patch.summary !== undefined
          ? mergeTaskLinks(existing.links, nextSummary)
          : existing.links;

    const updated: AssistantTask = {
      ...existing,
      title: patch.title !== undefined ? patch.title.trim() : existing.title,
      status: patch.status ?? existing.status,
      priority: patch.priority ?? existing.priority,
      source: patch.source ?? existing.source,
      summary: nextSummary,
      links: nextLinks,
      blockingQuestions:
        patch.blockingQuestions !== undefined
          ? patch.blockingQuestions.map((item) => item.trim()).filter(Boolean)
          : existing.blockingQuestions,
      updatedAt: new Date().toISOString(),
    };

    if (patch.projectId === null) delete updated.projectId;
    else if (patch.projectId !== undefined) updated.projectId = patch.projectId;

    if (patch.draftReply === null) delete updated.draftReply;
    else if (patch.draftReply !== undefined) {
      const trimmed = patch.draftReply.trim();
      if (trimmed) updated.draftReply = trimmed;
      else delete updated.draftReply;
    }

    if (patch.sessionId === null) delete updated.sessionId;
    else if (patch.sessionId !== undefined) updated.sessionId = patch.sessionId;

    this.#tasks.set(id, updated);
    this.#saveAndBroadcast();
    return updated;
  }

  delete(id: string): void {
    if (!this.#tasks.has(id)) throw new UserError("任务不存在");
    this.#tasks.delete(id);
    this.#saveAndBroadcast();
  }

  /** Transitions a running task to `review` automatically once its linked session finishes cleanly. */
  onSessionTransition(sessionId: string, previous: SessionStatus | undefined, next: SessionStatus): void {
    if (previous !== "running" || next !== "idle") return;
    let changed = false;
    const now = new Date().toISOString();
    for (const task of this.#tasks.values()) {
      if (task.sessionId !== sessionId || task.status !== "running") continue;
      task.status = "review";
      task.updatedAt = now;
      changed = true;
    }
    if (changed) this.#saveAndBroadcast();
  }

  /** Unbinds project references when a project is removed while keeping the task record intact. */
  onProjectRemoved(projectId: string): void {
    let changed = false;
    const now = new Date().toISOString();
    for (const task of this.#tasks.values()) {
      if (task.projectId !== projectId) continue;
      delete task.projectId;
      task.updatedAt = now;
      changed = true;
    }
    if (changed) this.#saveAndBroadcast();
  }

  /** Clears session references if the linked session is deleted. */
  forgetSession(sessionId: string): void {
    let changed = false;
    for (const task of this.#tasks.values()) {
      if (task.sessionId !== sessionId) continue;
      delete task.sessionId;
      changed = true;
    }
    if (changed) this.#saveAndBroadcast();
  }

  #load(): void {
    if (!existsSync(this.#path)) return;
    try {
      const parsed = JSON.parse(readFileSync(this.#path, "utf8")) as Partial<PersistedTasks>;
      if (!parsed || !Array.isArray(parsed.tasks)) throw new Error("tasks.json root is invalid");
      for (const task of parsed.tasks) {
        if (!task || typeof task.id !== "string" || typeof task.title !== "string") continue;
        this.#tasks.set(task.id, {
          ...task,
          summary: typeof task.summary === "string" ? task.summary : "",
          links: Array.isArray(task.links) ? task.links : [],
          blockingQuestions: Array.isArray(task.blockingQuestions) ? task.blockingQuestions : [],
        });
      }
    } catch (error) {
      const backup = `${this.#path}.corrupt.${Date.now()}`;
      try {
        renameSync(this.#path, backup);
        console.warn(`[core] corrupted tasks.json backed up to ${backup}`, error);
      } catch (backupError) {
        console.warn("[core] failed to back up corrupted tasks.json", backupError);
      }
    }
  }

  #saveAndBroadcast(): void {
    const payload: PersistedTasks = { version: 1, tasks: Array.from(this.#tasks.values()) };
    const tmp = `${this.#path}.${process.pid}.tmp`;
    writeFileSync(tmp, `${JSON.stringify(payload, null, 2)}\n`, { mode: 0o600 });
    renameSync(tmp, this.#path);
    this.#onChange(this.list());
  }
}

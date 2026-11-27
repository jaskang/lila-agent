import { randomUUID } from "node:crypto";
import type { AgentToolResult, ExtensionAPI, ExtensionFactory } from "@earendil-works/pi-coding-agent";
import type {
  ApprovalMode,
  AssistantTask,
  PendingAnswer,
  PendingRequest,
  ScheduleCreateInput,
  ScheduledJob,
  ScheduleUpdatePatch,
  TaskCreateInput,
  TaskUpdatePatch,
} from "@lila/protocol";
import { Type } from "typebox";
import { approvalReason, suggestApprovalRule } from "./approval-policy.ts";

export interface AutomationBridge {
  readonly defaultProjectId: string;
  listSchedules(): ScheduledJob[];
  createSchedule(input: ScheduleCreateInput): ScheduledJob;
  updateSchedule(id: string, patch: ScheduleUpdatePatch): ScheduledJob;
  deleteSchedule(id: string): void;
  listTasks(): AssistantTask[];
  upsertTask(input: { id?: string } & (TaskCreateInput | TaskUpdatePatch)): AssistantTask;
}

export type SubagentBusEvent =
  | { type: "created"; id: string; agentType: string; description: string; isBackground: boolean }
  | { type: "started"; id: string; agentType: string; description: string }
  | { type: "usage"; id: string; agentType: string; description: string; model?: string; depth?: number; tokens: number; cost: number }
  | { type: "compacted"; id: string; compactionCount?: number }
  | { type: "steered"; id: string; message?: string }
  | {
      type: "finished";
      id: string;
      agentType: string;
      description: string;
      status: string;
      toolUses?: number;
      durationMs?: number;
      tokens?: number;
      cost?: number;
      result?: string;
      error?: string;
    }
  | { type: "disposed" };

export interface SubagentController {
  stop(agentId: string): Promise<void>;
  steer(agentId: string, message: string): Promise<void>;
}

/** What the inline extension needs from the live session that hosts it. */
export interface LilaBridge {
  readonly approvalMode: ApprovalMode;
  readonly automation?: AutomationBridge | undefined;
  approvalRules(): readonly string[];
  allowRule(scope: "session" | "project", pattern: string): void;
  setSubagentController?(controller: SubagentController | undefined): void;
  request(request: Omit<PendingRequest, "id">, options?: { signal?: AbortSignal; timeout?: number }): Promise<PendingAnswer | undefined>;
  onSubagentEvent?(event: SubagentBusEvent): void;
  onProviderResponse?(providerId: string | undefined, status: number, headers: Record<string, string>): void;
}

const SUMMARY_LIMIT = 4_000;

function summarize(toolName: string, input: Record<string, unknown>): string {
  if ((toolName === "bash" || toolName === "powershell") && typeof input.command === "string") return input.command;
  if ((toolName === "edit" || toolName === "write") && typeof input.path === "string") return input.path;
  if (toolName === "memory_add" || toolName === "memory_replace" || toolName === "memory_remove") {
    const target = input.target === "user" ? "用户画像 (USER.md)" : "Agent 笔记 (MEMORY.md)";
    const parts = [`目标：${target}`];
    if (typeof input.oldText === "string" && input.oldText) parts.push(`匹配：${input.oldText}`);
    if (typeof input.content === "string" && input.content) parts.push(`内容：${input.content}`);
    if (typeof input.newContent === "string" && input.newContent) parts.push(`替换为：${input.newContent}`);
    return parts.join("\n");
  }
  const json = JSON.stringify(input, null, 2) ?? "";
  return json.length > SUMMARY_LIMIT ? `${json.slice(0, SUMMARY_LIMIT)}\n…` : json;
}

const AskUserParams = Type.Object({
  questions: Type.Array(
    Type.Object({
      id: Type.String({ description: "Short stable id for the question." }),
      question: Type.String(),
      options: Type.Array(Type.String(), { description: "Suggested answers; the user can also type their own." }),
      multiple: Type.Optional(Type.Boolean({ description: "Allow choosing several options." })),
    }),
    { minItems: 1, maxItems: 4 },
  ),
});

type AskUserDetails = { skipped?: boolean; answers?: Extract<PendingAnswer, { type: "ask" }>["answers"] };

function asRecord(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : undefined;
}

function asStr(value: unknown): string | undefined {
  return typeof value === "string" && value ? value : undefined;
}

function asNum(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function usageSummary(rawUsage: unknown, rawTokens?: unknown): { tokens: number; cost: number } {
  const usage = asRecord(rawUsage);
  const tokensRecord = asRecord(rawTokens);
  const costRecord = asRecord(usage?.cost);
  const tokens = asNum(tokensRecord?.total) ?? asNum(usage?.totalTokens) ?? ((asNum(usage?.input) ?? 0) + (asNum(usage?.output) ?? 0));
  const cost = asNum(costRecord?.total) ?? 0;
  return { tokens, cost };
}

/** Tool approvals, `ask_user`, and live subagent progress forwarding bound to one live session. */
export function createLilaExtension(bridge: LilaBridge, projectRoot: string): ExtensionFactory {
  return (pi: ExtensionAPI) => {
    const unsubs: Array<() => void> = [];
    if (bridge.onSubagentEvent) {
      const emit = bridge.onSubagentEvent.bind(bridge);
      unsubs.push(
        pi.events.on("subagents:created", (raw) => {
          const data = asRecord(raw);
          const id = asStr(data?.id);
          if (!id) return;
          emit({
            type: "created",
            id,
            agentType: asStr(data?.type) ?? "Agent",
            description: asStr(data?.description) ?? "",
            isBackground: Boolean(data?.isBackground),
          });
        }),
        pi.events.on("subagents:started", (raw) => {
          const data = asRecord(raw);
          const id = asStr(data?.id);
          if (!id) return;
          emit({
            type: "started",
            id,
            agentType: asStr(data?.type) ?? "Agent",
            description: asStr(data?.description) ?? "",
          });
        }),
        pi.events.on("subagents:usage", (raw) => {
          const data = asRecord(raw);
          const id = asStr(data?.id);
          if (!id) return;
          const depth = asNum(data?.depth);
          const model = asStr(data?.model);
          const { tokens, cost } = usageSummary(data?.usage);
          emit({
            type: "usage",
            id,
            agentType: asStr(data?.type) ?? "Agent",
            description: asStr(data?.description) ?? "",
            ...(model ? { model } : {}),
            ...(depth !== undefined ? { depth } : {}),
            tokens,
            cost,
          });
        }),
        pi.events.on("subagents:compacted", (raw) => {
          const data = asRecord(raw);
          const id = asStr(data?.id);
          if (!id) return;
          const compactionCount = asNum(data?.compactionCount);
          emit({ type: "compacted", id, ...(compactionCount !== undefined ? { compactionCount } : {}) });
        }),
        pi.events.on("subagents:steered", (raw) => {
          const data = asRecord(raw);
          const id = asStr(data?.id);
          if (!id) return;
          const message = asStr(data?.message);
          emit({ type: "steered", id, ...(message ? { message } : {}) });
        }),
        pi.events.on("subagents:completed", (raw) => {
          const data = asRecord(raw);
          const id = asStr(data?.id);
          if (!id) return;
          const { tokens, cost } = usageSummary(data?.usage, data?.tokens);
          const toolUses = asNum(data?.toolUses);
          const durationMs = asNum(data?.durationMs);
          const result = asStr(data?.result);
          emit({
            type: "finished",
            id,
            agentType: asStr(data?.type) ?? "Agent",
            description: asStr(data?.description) ?? "",
            status: asStr(data?.status) ?? "completed",
            ...(toolUses !== undefined ? { toolUses } : {}),
            ...(durationMs !== undefined ? { durationMs } : {}),
            ...(tokens > 0 ? { tokens } : {}),
            ...(cost > 0 ? { cost } : {}),
            ...(result ? { result: result.length > 8_000 ? `${result.slice(0, 8_000)}\n…` : result } : {}),
          });
        }),
        pi.events.on("subagents:failed", (raw) => {
          const data = asRecord(raw);
          const id = asStr(data?.id);
          if (!id) return;
          const { tokens, cost } = usageSummary(data?.usage, data?.tokens);
          const toolUses = asNum(data?.toolUses);
          const durationMs = asNum(data?.durationMs);
          const result = asStr(data?.result);
          const error = asStr(data?.error);
          emit({
            type: "finished",
            id,
            agentType: asStr(data?.type) ?? "Agent",
            description: asStr(data?.description) ?? "",
            status: asStr(data?.status) ?? "error",
            ...(toolUses !== undefined ? { toolUses } : {}),
            ...(durationMs !== undefined ? { durationMs } : {}),
            ...(tokens > 0 ? { tokens } : {}),
            ...(cost > 0 ? { cost } : {}),
            ...(result ? { result: result.length > 8_000 ? `${result.slice(0, 8_000)}\n…` : result } : {}),
            ...(error ? { error } : {}),
          });
        }),
        pi.events.on("subagents:disposed", () => emit({ type: "disposed" })),
      );
      pi.on("session_shutdown", () => {
        bridge.setSubagentController?.(undefined);
        for (const unsub of unsubs.splice(0)) unsub();
      });
    }

    if (bridge.setSubagentController) {
      const rpcCall = (channel: string, payload: Record<string, unknown>): Promise<void> =>
        new Promise((resolve, reject) => {
          const requestId = randomUUID();
          const replyChannel = `${channel}:reply:${requestId}`;
          let settled = false;
          const finish = (err?: Error) => {
            if (settled) return;
            settled = true;
            clearTimeout(timer);
            unsub();
            if (err) reject(err);
            else resolve();
          };
          const unsub = pi.events.on(replyChannel, (raw) => {
            const reply = asRecord(raw);
            if (reply?.success === true) finish();
            else finish(new Error(asStr(reply?.error) ?? "操作子 Agent 失败"));
          });
          const timer = setTimeout(() => finish(new Error("子 Agent 扩展未响应")), 5_000);
          timer.unref();
          pi.events.emit(channel, { requestId, ...payload });
        });

      bridge.setSubagentController({
        stop: (agentId) => rpcCall("subagents:rpc:stop", { agentId }),
        steer: (agentId, message) => rpcCall("subagents:rpc:steer", { agentId, message }),
      });
    }

    if (bridge.onProviderResponse) {
      const onResponse = bridge.onProviderResponse.bind(bridge);
      pi.on("after_provider_response", (event, ctx) => {
        onResponse(ctx.model?.provider, event.status, event.headers);
      });
    }

    pi.on("tool_call", async (event, ctx) => {
      const mode = bridge.approvalMode;
      if (mode === "auto") return;
      const name = event.toolName;
      const input = event.input as Record<string, unknown>;
      const tool = pi.getAllTools().find((candidate) => candidate.name === name);
      const reason = approvalReason({
        mode,
        projectRoot,
        cwd: ctx.cwd,
        name,
        input,
        tool,
        rules: bridge.approvalRules(),
      });
      if (!reason) return;

      const ruleSuggestion = suggestApprovalRule(name, input, tool);
      const answer = await bridge.request(
        {
          kind: "approval",
          title: `运行 ${name}`,
          message: `${reason}\n\n${summarize(name, input)}`,
          options: ["allow", "deny"],
          toolCallId: event.toolCallId,
          toolName: name,
          ...(ruleSuggestion ? { ruleSuggestion } : {}),
        },
        ctx.signal ? { signal: ctx.signal } : {},
      );
      if (answer?.type === "option" && (answer.value === "allow" || answer.value === "allow-session" || answer.value === "allow-project")) {
        if (ruleSuggestion && answer.value === "allow-session") bridge.allowRule("session", ruleSuggestion.pattern);
        else if (ruleSuggestion && answer.value === "allow-project") bridge.allowRule("project", ruleSuggestion.pattern);
        return;
      }
      if (answer?.type === "option") {
        const note = answer.note?.trim();
        return {
          block: true,
          reason: note ? `用户拒绝了这次工具调用，并补充说明：${note}` : "用户拒绝了这次工具调用。",
        };
      }
      return { block: true, reason: "用户取消了这次工具调用。" };
    });

    pi.registerTool({
      name: "ask_user",
      label: "询问用户",
      description:
        "Ask the user one to four short questions and wait for the answers. Use it to clarify unclear requirements or to let the user choose between options instead of guessing. Offer concrete options; the user can also answer in their own words.",
      promptSnippet: "Ask the user clarifying questions with suggested options",
      parameters: AskUserParams,
      async execute(_toolCallId, params, signal): Promise<AgentToolResult<AskUserDetails>> {
        const questions = params.questions.map((question) => ({ ...question, multiple: question.multiple ?? false }));
        const answer = await bridge.request(
          { kind: "ask", title: "需要你的回答", options: [], questions },
          signal ? { signal } : {},
        );
        if (answer?.type !== "ask") return { content: [{ type: "text", text: "用户跳过了问题。" }], details: { skipped: true } };
        const text = questions
          .map((question) => {
            const reply = answer.answers.find((item) => item.id === question.id);
            const parts = [...(reply?.selected ?? []), ...(reply?.custom?.trim() ? [reply.custom.trim()] : [])];
            return `Q: ${question.question}\nA: ${parts.length ? parts.join("; ") : "（未回答）"}`;
          })
          .join("\n\n");
        return { content: [{ type: "text", text }], details: { answers: answer.answers } };
      },
    });

    const automation = bridge.automation;
    if (automation) {
      pi.registerTool({
        name: "schedule_list",
        label: "查看定时任务",
        description: "List all persistent Lila scheduled jobs (cron, interval, and one-shot) along with their enabled state, next run time, and last status.",
        promptSnippet: "List persistent scheduled jobs",
        parameters: Type.Object({}),
        async execute() {
          const jobs = automation.listSchedules();
          if (!jobs.length) return { content: [{ type: "text", text: "当前没有任何定时任务。" }], details: { count: 0 } };
          const text = jobs
            .map(
              (job) =>
                `- [${job.enabled ? "启用" : "停用"}] ${job.name} (id: ${job.id}) | 规则: ${job.schedule} (${job.scheduleKind}) | 模式: ${job.sessionMode} | 状态: ${job.lastStatus}${job.nextRunAt ? ` | 下次: ${job.nextRunAt}` : ""}`,
            )
            .join("\n");
          return { content: [{ type: "text", text }], details: { count: jobs.length } };
        },
      });

      pi.registerTool({
        name: "schedule_create",
        label: "创建定时任务",
        description:
          "Create a persistent background scheduled job that survives session close and core restarts. Supports 5/6-field cron (e.g. '0 9 * * 1-5'), fixed interval ('15m', '1h', '1d'), or one-shot ('+30m' or ISO timestamp).",
        promptSnippet: "Create a persistent scheduled job",
        parameters: Type.Object({
          name: Type.String({ description: "Short descriptive name for the job." }),
          schedule: Type.String({ description: "5/6-field cron, interval like '1h'/'15m', or one-shot like '+30m' or ISO timestamp." }),
          prompt: Type.String({ description: "The exact prompt to send to the agent when this schedule fires." }),
          sessionMode: Type.Optional(Type.Union([Type.Literal("new"), Type.Literal("reuse")], { description: "Default 'new' creates a clean session each run; 'reuse' appends to one persistent session." })),
          approvalMode: Type.Optional(Type.Union([Type.Literal("ask-all"), Type.Literal("ask-exec"), Type.Literal("auto")])),
          enabled: Type.Optional(Type.Boolean()),
        }),
        async execute(_toolCallId, params) {
          const job = automation.createSchedule({
            name: params.name,
            projectId: automation.defaultProjectId,
            schedule: params.schedule,
            prompt: params.prompt,
            ...(params.sessionMode ? { sessionMode: params.sessionMode } : {}),
            ...(params.approvalMode ? { approvalMode: params.approvalMode } : {}),
            ...(params.enabled !== undefined ? { enabled: params.enabled } : {}),
          });
          return {
            content: [
              {
                type: "text",
                text: `已创建定时任务「${job.name}」(id: ${job.id})，调度规则：${job.schedule}${job.nextRunAt ? `，下次运行：${job.nextRunAt}` : ""}。`,
              },
            ],
            details: { id: job.id },
          };
        },
      });

      pi.registerTool({
        name: "schedule_update",
        label: "更新定时任务",
        description: "Update an existing persistent scheduled job (enable/disable, rename, change schedule expression, prompt, or session mode).",
        promptSnippet: "Update or toggle a scheduled job",
        parameters: Type.Object({
          id: Type.String({ description: "Scheduled job id from schedule_list." }),
          name: Type.Optional(Type.String()),
          schedule: Type.Optional(Type.String()),
          prompt: Type.Optional(Type.String()),
          enabled: Type.Optional(Type.Boolean()),
          sessionMode: Type.Optional(Type.Union([Type.Literal("new"), Type.Literal("reuse")])),
          approvalMode: Type.Optional(Type.Union([Type.Literal("ask-all"), Type.Literal("ask-exec"), Type.Literal("auto")])),
        }),
        async execute(_toolCallId, params) {
          const { id, ...patch } = params;
          const job = automation.updateSchedule(id, patch);
          return {
            content: [
              {
                type: "text",
                text: `已更新定时任务「${job.name}」(状态：${job.enabled ? "启用" : "停用"}，规则：${job.schedule}${job.nextRunAt ? `，下次运行：${job.nextRunAt}` : ""})。`,
              },
            ],
            details: { id: job.id },
          };
        },
      });

      pi.registerTool({
        name: "schedule_delete",
        label: "删除定时任务",
        description: "Delete a persistent scheduled job by id.",
        promptSnippet: "Delete a scheduled job",
        parameters: Type.Object({
          id: Type.String({ description: "Scheduled job id to remove." }),
        }),
        async execute(_toolCallId, params) {
          automation.deleteSchedule(params.id);
          return { content: [{ type: "text", text: `已删除定时任务 ${params.id}。` }], details: { id: params.id } };
        },
      });

      pi.registerTool({
        name: "task_list",
        label: "查看任务收件箱",
        description: "List tasks in Lila's personal assistant task inbox with their status, priority, source, links, blocking questions, and draft replies.",
        promptSnippet: "List assistant tasks",
        parameters: Type.Object({}),
        async execute() {
          const tasks = automation.listTasks();
          if (!tasks.length) return { content: [{ type: "text", text: "任务收件箱为空。" }], details: { count: 0 } };
          const text = tasks
            .map(
              (task) =>
                `- [${task.status}] ${task.title} (id: ${task.id}, 来源: ${task.source}, 优先级: ${task.priority})${task.links.length ? ` | 链接: ${task.links.map((l) => l.url).join(", ")}` : ""}${task.blockingQuestions.length ? ` | 待澄清: ${task.blockingQuestions.length}项` : ""}`,
            )
            .join("\n");
          return { content: [{ type: "text", text }], details: { count: tasks.length } };
        },
      });

      pi.registerTool({
        name: "task_upsert",
        label: "记录/更新任务",
        description:
          "Create a new task in Lila's task inbox or update an existing task (provide `id` to update). Use this to capture requirements from chats/URLs, record `blockingQuestions` when entering `clarifying`, or save an outbound `draftReply` when moving to `review`.",
        promptSnippet: "Create or update an assistant task",
        parameters: Type.Object({
          id: Type.Optional(Type.String({ description: "Existing task id to update; omit to create a new task." })),
          title: Type.Optional(Type.String({ description: "Required when creating a new task." })),
          status: Type.Optional(
            Type.Union([
              Type.Literal("inbox"),
              Type.Literal("clarifying"),
              Type.Literal("running"),
              Type.Literal("review"),
              Type.Literal("done"),
            ]),
          ),
          priority: Type.Optional(Type.Union([Type.Literal("low"), Type.Literal("normal"), Type.Literal("high")])),
          source: Type.Optional(
            Type.Union([
              Type.Literal("manual"),
              Type.Literal("wechat"),
              Type.Literal("whatsapp"),
              Type.Literal("github"),
              Type.Literal("asana"),
              Type.Literal("schedule"),
            ]),
          ),
          summary: Type.Optional(Type.String({ description: "Structured requirement summary or pasted context." })),
          links: Type.Optional(
            Type.Array(
              Type.Object({
                url: Type.String(),
                label: Type.Optional(Type.String()),
                kind: Type.Optional(
                  Type.Union([
                    Type.Literal("design"),
                    Type.Literal("issue"),
                    Type.Literal("pr"),
                    Type.Literal("doc"),
                    Type.Literal("other"),
                  ]),
                ),
              }),
            ),
          ),
          blockingQuestions: Type.Optional(Type.Array(Type.String(), { description: "Open questions needed from stakeholders before proceeding." })),
          draftReply: Type.Optional(Type.String({ description: "Draft reply for the user to review before sending externally." })),
        }),
        async execute(_toolCallId, params) {
          const task = automation.upsertTask(params as { id?: string } & (TaskCreateInput | TaskUpdatePatch));
          return {
            content: [{ type: "text", text: `任务已保存：「${task.title}」(id: ${task.id}，状态：${task.status})。` }],
            details: { id: task.id, status: task.status },
          };
        },
      });
    }
  };
}

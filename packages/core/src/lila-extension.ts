import type { AgentToolResult, ExtensionAPI, ExtensionFactory } from "@earendil-works/pi-coding-agent";
import type { ApprovalMode, PendingAnswer, PendingRequest } from "@lila/protocol";
import { Type } from "typebox";

/** What the inline extension needs from the live session that hosts it. */
export interface LilaBridge {
  readonly approvalMode: ApprovalMode;
  /** Tools the user answered "always" for, in this session. */
  readonly allowedTools: Set<string>;
  request(request: Omit<PendingRequest, "id">, options?: { signal?: AbortSignal; timeout?: number }): Promise<PendingAnswer | undefined>;
}

const READ_ONLY_TOOLS: Record<string, true> = {
  read: true,
  grep: true,
  find: true,
  ls: true,
  list_mcp_resources: true,
  list_mcp_resource_templates: true,
  read_mcp_resource: true,
  tool_search: true,
  get_subagent_result: true,
  ask_user: true,
};
const FILE_TOOLS: Record<string, true> = { edit: true, write: true };
const SUMMARY_LIMIT = 4_000;

function summarize(toolName: string, input: Record<string, unknown>): string {
  if (toolName === "bash" && typeof input.command === "string") return input.command;
  if (FILE_TOOLS[toolName] && typeof input.path === "string") return input.path;
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

/** Tool approvals and the `ask_user` tool, bound to one live session. */
export function createLilaExtension(bridge: LilaBridge): ExtensionFactory {
  return (pi: ExtensionAPI) => {
    pi.on("tool_call", async (event, ctx) => {
      // Calls a tool makes (codemode scripts) follow the decision about the calling tool.
      if (event.parentToolCallId) return;
      const mode = bridge.approvalMode;
      const name = event.toolName;
      if (mode === "auto" || READ_ONLY_TOOLS[name] || bridge.allowedTools.has(name)) return;
      if (mode === "ask-exec" && FILE_TOOLS[name]) return;
      if (pi.getAllTools().find((tool) => tool.name === name)?.annotations?.readOnlyHint) return;

      const answer = await bridge.request(
        {
          kind: "approval",
          title: `运行 ${name}`,
          message: summarize(name, event.input as Record<string, unknown>),
          options: ["allow", "always", "deny"],
          toolCallId: event.toolCallId,
          toolName: name,
        },
        ctx.signal ? { signal: ctx.signal } : {},
      );
      if (answer?.type === "option" && answer.value === "allow") return;
      if (answer?.type === "option" && answer.value === "always") {
        bridge.allowedTools.add(name);
        return;
      }
      return { block: true, reason: answer?.type === "option" ? "用户拒绝了这次工具调用。" : "用户取消了这次工具调用。" };
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
  };
}

import { useEffect, useRef, useState } from "react";
import {
  ArrowLeft,
  Check,
  CircleAlert,
  LoaderCircle,
  Plus,
  Send,
  Square,
  X,
} from "lucide-react";
import {
  DEFAULT_CORE_PORT,
  ProtocolMessage,
  type ApprovalMode,
  type ClientCommand,
  type SessionEvent,
} from "@lila/protocol";
import { Button } from "@/components/ui/button";

type ConnectionState = "connecting" | "connected" | "disconnected";
type Project = { id: string; path: string };
type Model = { id: string; label: string };
type SessionSummary = {
  id: string;
  projectId: string;
  modelId?: string;
  approvalMode: ApprovalMode;
  status: string;
};
type PendingQuestion = {
  id: string;
  question: string;
  options: string[];
  multiple: boolean;
};
type PendingItem = {
  id: string;
  method: string;
  title: string;
  message?: string;
  options: string[];
  questions: PendingQuestion[];
  placeholder?: string;
  defaultValue?: string;
};
type ToolEntry = {
  kind: "tool";
  id: string;
  toolCallId: string;
  toolName: string;
  args: unknown;
  intent?: string;
  output?: string;
  result?: string;
  isError?: boolean;
  files?: unknown;
};
type ChatEntry =
  | { kind: "user"; id: string; text: string }
  | {
      kind: "assistant";
      id: string;
      messageId: string;
      text: string;
      thinking: string;
      runId?: string;
      completed?: boolean;
    }
  | ToolEntry;
type PendingDraft = {
  value?: string;
  selectedOptions?: Record<string, string[]>;
  customInput?: Record<string, string>;
};

function id(): string {
  return crypto.randomUUID();
}

function asText(value: unknown): string {
  if (typeof value === "string") return value;
  if (value === undefined) return "";
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

function field(payload: Record<string, unknown>, key: string): string | undefined {
  return typeof payload[key] === "string" ? payload[key] : undefined;
}

function parseQuestions(value: unknown): PendingQuestion[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((item, index) => {
    if (!item || typeof item !== "object") return [];
    const raw = item as Record<string, unknown>;
    const questionId = typeof raw.id === "string" ? raw.id : `question-${index}`;
    const question = typeof raw.question === "string" ? raw.question : "请选择";
    const options = Array.isArray(raw.options)
      ? raw.options.filter((option): option is string => typeof option === "string")
      : [];
    return [
      {
        id: questionId,
        question,
        options,
        multiple: raw.multiSelect === true || raw.multiple === true || raw.allowMultiple === true,
      },
    ];
  });
}

function projectFrom(value: unknown): Project | undefined {
  if (!value || typeof value !== "object") return undefined;
  const item = value as Record<string, unknown>;
  return typeof item.id === "string" && typeof item.path === "string"
    ? { id: item.id, path: item.path }
    : undefined;
}

function modelFrom(value: unknown): Model | undefined {
  if (!value || typeof value !== "object") return undefined;
  const item = value as Record<string, unknown>;
  return typeof item.id === "string" && typeof item.label === "string"
    ? { id: item.id, label: item.label }
    : undefined;
}

function sessionFrom(value: unknown): SessionSummary | undefined {
  if (!value || typeof value !== "object") return undefined;
  const item = value as Record<string, unknown>;
  if (
    typeof item.id !== "string" ||
    typeof item.projectId !== "string" ||
    typeof item.approvalMode !== "string" ||
    typeof item.status !== "string"
  ) {
    return undefined;
  }
  return {
    id: item.id,
    projectId: item.projectId,
    modelId: typeof item.modelId === "string" ? item.modelId : undefined,
    approvalMode: item.approvalMode as ApprovalMode,
    status: item.status,
  };
}

function readStoredSession(): string | undefined {
  try {
    return localStorage.getItem("lila.currentSessionId") || undefined;
  } catch {
    return undefined;
  }
}

function writeStoredSession(sessionId: string | undefined): void {
  try {
    if (sessionId) localStorage.setItem("lila.currentSessionId", sessionId);
    else localStorage.removeItem("lila.currentSessionId");
  } catch {
    // Storage is optional in embedded renderer contexts.
  }
}

export function App() {
  const socketRef = useRef<WebSocket | null>(null);
  const sessionRef = useRef<string | undefined>(readStoredSession());
  const seenSeqRef = useRef<Record<string, number>>({});
  const creatingSessionRef = useRef(false);
  const pendingCreateRequestRef = useRef<string | undefined>(undefined);
  const activeRunRef = useRef<string | undefined>(undefined);
  const [connection, setConnection] = useState<ConnectionState>("connecting");
  const [projects, setProjects] = useState<Project[]>([]);
  const [models, setModels] = useState<Model[]>([]);
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [projectId, setProjectId] = useState("");
  const [newProjectPath, setNewProjectPath] = useState("");
  const [modelId, setModelId] = useState("");
  const [approvalMode, setApprovalMode] = useState<ApprovalMode>("yolo");
  const [sessionId, setSessionId] = useState<string | undefined>(sessionRef.current);
  const [lastEventSeq, setLastEventSeq] = useState(0);
  const [entries, setEntries] = useState<ChatEntry[]>([]);
  const [composer, setComposer] = useState("");
  const [pending, setPending] = useState<PendingItem[]>([]);
  const [pendingDrafts, setPendingDrafts] = useState<Record<string, PendingDraft>>({});
  const [activeRunId, setActiveRunId] = useState<string | undefined>();
  const [status, setStatus] = useState("等待连接");
  const [error, setError] = useState<string>();

  function sendCommand(command: ClientCommand): void {
    const socket = socketRef.current;
    if (socket?.readyState === WebSocket.OPEN) socket.send(JSON.stringify(command));
  }

  function resumeSession(nextSessionId: string, fromSeq = 0): void {
    sendCommand({ type: "events.resume", sessionId: nextSessionId, lastEventSeq: fromSeq });
  }

  function clearSessionView(): void {
    setEntries([]);
    setPending([]);
    setPendingDrafts({});
    setActiveRunId(undefined);
    activeRunRef.current = undefined;
    setLastEventSeq(0);
    setError(undefined);
  }

  function activateSession(nextSessionId: string, shouldResume: boolean): void {
    sessionRef.current = nextSessionId;
    setSessionId(nextSessionId);
    writeStoredSession(nextSessionId);
    seenSeqRef.current[nextSessionId] = 0;
    clearSessionView();
    if (shouldResume) resumeSession(nextSessionId, 0);
  }

  function handleResponse(message: Extract<ProtocolMessage, { type: "response" }>): void {
    if (!message.success) {
      setError(message.error || `${message.command} 失败`);
      setStatus("请求失败");
      return;
    }
    const data = message.data ?? {};
    if (message.command === "projects.list") {
      const next = Array.isArray(data.projects) ? data.projects.flatMap((item) => {
        const project = projectFrom(item);
        return project ? [project] : [];
      }) : [];
      setProjects(next);
      setProjectId((current) => current && next.some((project) => project.id === current) ? current : next[0]?.id || "");
    } else if (message.command === "projects.add") {
      const project = projectFrom(data.project);
      if (project) {
        setProjects((current) => current.some((item) => item.id === project.id) ? current : [...current, project]);
        setProjectId(project.id);
        setNewProjectPath("");
        setStatus("项目已添加");
      }
    } else if (message.command === "models.list") {
      const next = Array.isArray(data.models) ? data.models.flatMap((item) => {
        const model = modelFrom(item);
        return model ? [model] : [];
      }) : [];
      setModels(next);
    } else if (message.command === "sessions.list") {
      const next = Array.isArray(data.sessions) ? data.sessions.flatMap((item) => {
        const session = sessionFrom(item);
        return session ? [session] : [];
      }) : [];
      setSessions(next);
      if (sessionRef.current && next.some((session) => session.id === sessionRef.current)) {
        const current = next.find((session) => session.id === sessionRef.current);
        if (current) {
          setProjectId(current.projectId);
          setModelId(current.modelId || "");
          setApprovalMode(current.approvalMode);
        }
      }
    } else if (message.command === "session.create") {
      const createdId = typeof data.sessionId === "string" ? data.sessionId : undefined;
      if (createdId && sessionRef.current !== createdId) {
        creatingSessionRef.current = false;
        activateSession(createdId, true);
      } else if (createdId) {
        creatingSessionRef.current = false;
      }
      pendingCreateRequestRef.current = undefined;
      setStatus("会话已创建");
    } else if (message.command === "session.configure") {
      const configured = sessionFrom(data.session);
      if (configured) {
        setModelId(configured.modelId || "");
        setApprovalMode(configured.approvalMode);
        setSessions((current) => current.map((session) => session.id === configured.id ? configured : session));
      }
      setStatus("会话设置已更新");
    }
  }

  useEffect(() => {
    let disposed = false;
    let retryTimer: number | undefined;
    const connect = () => {
      if (disposed) return;
      setConnection("connecting");
      const socket = new WebSocket(`ws://127.0.0.1:${DEFAULT_CORE_PORT}/ws`);
      socketRef.current = socket;
      socket.onopen = () => setStatus("正在握手…");
      socket.onmessage = (raw) => {
        try {
          const parsed = ProtocolMessage.safeParse(JSON.parse(String(raw.data)));
          if (!parsed.success) return;
          const message = parsed.data;
          if (message.type === "hello") {
            setConnection("connected");
            setError(undefined);
            setStatus("已连接");
            sendCommand({ type: "projects.list", requestId: id() });
            sendCommand({ type: "models.list", requestId: id() });
            sendCommand({ type: "sessions.list", requestId: id() });
            if (sessionRef.current) resumeSession(sessionRef.current, 0);
          } else if (message.type === "response") {
            handleResponse(message);
          } else if (message.type === "error") {
            if (message.code === "session_not_found" && sessionRef.current) activateSession("", false);
            setError(message.message);
            setStatus("core 返回错误");
          } else {
            handleEvent(message);
          }
        } catch {
          setError("无法解析 core 消息");
        }
      };
      socket.onerror = () => {
        setConnection("disconnected");
        setStatus("core 连接失败");
      };
      socket.onclose = () => {
        if (socketRef.current !== socket) return;
        socketRef.current = null;
        setConnection("disconnected");
        setStatus("core 已断开，正在重连…");
        if (!disposed) retryTimer = window.setTimeout(connect, 1500);
      };
    };
    connect();
    return () => {
      disposed = true;
      if (retryTimer !== undefined) window.clearTimeout(retryTimer);
      if (socketRef.current) socketRef.current.close();
      socketRef.current = null;
    };
  }, []);

  function handleEvent(event: SessionEvent): void {
    const currentSession = sessionRef.current;
    if (
      event.eventType === "session.created" &&
      !currentSession &&
      creatingSessionRef.current &&
      event.requestId === pendingCreateRequestRef.current
    ) {
      activateSession(event.sessionId, false);
      creatingSessionRef.current = false;
      pendingCreateRequestRef.current = undefined;
    } else if (!currentSession || event.sessionId !== currentSession) {
      return;
    }
    const seen = seenSeqRef.current[event.sessionId] ?? 0;
    if (event.seq <= seen) return;
    seenSeqRef.current[event.sessionId] = event.seq;
    setLastEventSeq(event.seq);
    const payload = event.payload;
    if (event.eventType === "session.created") {
      setStatus("会话已创建");
    } else if (event.eventType === "session.updated") {
      const nextModelId = field(payload, "modelId") || "";
      const nextApprovalMode = field(payload, "approvalMode");
      setModelId(nextModelId);
      if (nextApprovalMode === "yolo" || nextApprovalMode === "write" || nextApprovalMode === "always-ask") {
        setApprovalMode(nextApprovalMode);
      }
      setSessions((current) => current.map((session) => session.id === event.sessionId
        ? { ...session, modelId: nextModelId || undefined, ...(nextApprovalMode ? { approvalMode: nextApprovalMode as ApprovalMode } : {}) }
        : session));
      setStatus("会话设置已更新");
    } else if (event.eventType === "message.user") {
      setEntries((current) => [...current, { kind: "user", id: `user-${event.seq}`, text: field(payload, "text") || asText(payload.message) }]);
    } else if (event.eventType === "run.started") {
      setActiveRunId(event.runId);
      activeRunRef.current = event.runId;
      setStatus("运行中");
      setError(undefined);
    } else if (event.eventType === "assistant.delta") {
      const messageId = field(payload, "messageId") || `${event.runId || "run"}-${field(payload, "kind") || "assistant"}`;
      const kind = field(payload, "kind");
      const delta = field(payload, "delta") || "";
      setEntries((current) => {
        const index = current.findIndex((entry) => entry.kind === "assistant" && entry.messageId === messageId);
        if (index < 0) {
          return [...current, {
            kind: "assistant",
            id: id(),
            messageId,
            text: kind === "text_delta" ? delta : "",
            thinking: kind === "thinking_delta" ? delta : "",
            runId: event.runId,
          }];
        }
        return current.map((entry, entryIndex) => {
          if (entryIndex !== index || entry.kind !== "assistant") return entry;
          return kind === "thinking_delta"
            ? { ...entry, thinking: entry.thinking + delta }
            : { ...entry, text: entry.text + delta };
        });
      });
    } else if (event.eventType === "assistant.completed") {
      const message = payload.message && typeof payload.message === "object" ? payload.message as Record<string, unknown> : undefined;
      const messageId = field(payload, "messageId") || (message && typeof message.id === "string" ? message.id : undefined) || `${event.runId || "run"}-assistant`;
      const content = message && Array.isArray(message.content) ? message.content : [];
      const text = content.flatMap((block) => {
        if (!block || typeof block !== "object") return [];
        const value = block as Record<string, unknown>;
        return value.type === "text" && typeof value.text === "string" ? [value.text] : [];
      }).join("");
      const thinking = content.flatMap((block) => {
        if (!block || typeof block !== "object") return [];
        const value = block as Record<string, unknown>;
        return value.type === "thinking" && typeof value.thinking === "string" ? [value.thinking] : [];
      }).join("");
      setEntries((current) => {
        const index = current.findIndex((entry) => entry.kind === "assistant" && entry.messageId === messageId);
        const finalText = text || (message && typeof message.text === "string" ? message.text : undefined);
        if (index < 0) return [...current, { kind: "assistant", id: id(), messageId, text: finalText || "", thinking, runId: event.runId, completed: true }];
        return current.map((entry, entryIndex) => entryIndex === index && entry.kind === "assistant"
          ? { ...entry, text: finalText ?? entry.text, thinking: thinking || entry.thinking, completed: true }
          : entry);
      });
    } else if (event.eventType === "tool.started") {
      const toolCallId = field(payload, "toolCallId") || `tool-${event.seq}`;
      setEntries((current) => current.some((entry) => entry.kind === "tool" && entry.toolCallId === toolCallId)
        ? current
        : [...current, {
            kind: "tool",
            id: id(),
            toolCallId,
            toolName: field(payload, "toolName") || "tool",
            args: payload.args,
            intent: field(payload, "intent"),
            output: "",
          }]);
      setStatus("工具执行中");
    } else if (event.eventType === "tool.output" || event.eventType === "tool.finished") {
      const toolCallId = field(payload, "toolCallId");
      if (toolCallId) {
        setEntries((current) => current.map((entry) => {
          if (entry.kind !== "tool" || entry.toolCallId !== toolCallId) return entry;
          const result = payload.result ?? payload.output ?? payload.partialResult;
          return {
            ...entry,
            output: event.eventType === "tool.output" && typeof result === "string" && entry.output ? `${entry.output}${result}` : asText(result),
            result: event.eventType === "tool.finished" ? asText(result) : entry.result,
            isError: typeof payload.isError === "boolean" ? payload.isError : entry.isError,
            files: payload.files ?? entry.files,
          };
        }));
      }
      if (event.eventType === "tool.finished") setStatus("工具已完成");
    } else if (event.eventType === "pending.created" && event.pendingId) {
      const method = field(payload, "method") || "input";
      const item: PendingItem = {
        id: event.pendingId,
        method,
        title: field(payload, "title") || (method === "ask" ? "需要回答问题" : "需要你的处理"),
        message: field(payload, "message"),
        options: Array.isArray(payload.options) ? payload.options.filter((option): option is string => typeof option === "string") : [],
        questions: parseQuestions(payload.questions),
        placeholder: field(payload, "placeholder"),
        defaultValue: field(payload, "defaultValue"),
      };
      setPending((current) => current.some((pendingItem) => pendingItem.id === item.id) ? current : [...current, item]);
      setStatus("等待你的回答");
    } else if (event.eventType === "pending.resolved" && event.pendingId) {
      setPending((current) => current.filter((item) => item.id !== event.pendingId));
      setPendingDrafts((current) => {
        const next = { ...current };
        delete next[event.pendingId as string];
        return next;
      });
    } else if (event.eventType === "run.finished") {
      setActiveRunId(undefined);
      activeRunRef.current = undefined;
      setStatus("运行完成");
    } else if (event.eventType === "run.aborted") {
      setActiveRunId(undefined);
      activeRunRef.current = undefined;
      setStatus("运行已中断");
    } else if (event.eventType === "run.failed" || event.eventType === "run.interrupted") {
      setActiveRunId(undefined);
      activeRunRef.current = undefined;
      const eventError = field(payload, "error") || asText(payload);
      setError(eventError);
      setStatus(event.eventType === "run.failed" ? "运行失败" : "运行被中断");
    }
  }

  function addProject(): void {
    const path = newProjectPath.trim();
    if (!path.startsWith("/")) {
      setError("请输入绝对目录路径");
      return;
    }
    sendCommand({ type: "projects.add", requestId: id(), path });
  }

  function createSession(): void {
    if (!projectId || connection !== "connected") return;
    creatingSessionRef.current = true;
    const requestId = id();
    pendingCreateRequestRef.current = requestId;
    sessionRef.current = undefined;
    setSessionId(undefined);
    writeStoredSession(undefined);
    clearSessionView();
    sendCommand({
      type: "session.create",
      requestId,
      projectId,
      ...(modelId ? { modelId } : {}),
      approvalMode,
    });
    setStatus("正在创建会话…");
  }

  function selectSession(next: SessionSummary): void {
    if (next.id === sessionRef.current) return;
    setProjectId(next.projectId);
    setModelId(next.modelId || "");
    setApprovalMode(next.approvalMode);
    activateSession(next.id, true);
  }

  function configureSession(nextModelId: string, nextApprovalMode: ApprovalMode): void {
    const currentSessionId = sessionRef.current;
    if (!currentSessionId || activeRunRef.current || connection !== "connected") return;
    sendCommand({
      type: "session.configure",
      requestId: id(),
      sessionId: currentSessionId,
      modelId: nextModelId || null,
      approvalMode: nextApprovalMode,
    });
  }

  function sendMessage(): void {
    const text = composer.trim();
    const currentSessionId = sessionRef.current;
    if (!currentSessionId || !text || activeRunRef.current || connection !== "connected") return;
    sendCommand({ type: "message.send", requestId: id(), sessionId: currentSessionId, text });
    setComposer("");
  }

  function abortRun(): void {
    const currentSessionId = sessionRef.current;
    const currentRunId = activeRunRef.current;
    if (!currentSessionId || !currentRunId) return;
    sendCommand({ type: "run.abort", requestId: id(), sessionId: currentSessionId, runId: currentRunId });
  }

  function answerPending(item: PendingItem, answer: unknown): void {
    const currentSessionId = sessionRef.current;
    if (!currentSessionId) return;
    sendCommand({ type: "pending.respond", requestId: id(), sessionId: currentSessionId, pendingId: item.id, answer });
  }

  function setDraft(itemId: string, update: PendingDraft): void {
    setPendingDrafts((current) => ({ ...current, [itemId]: { ...current[itemId], ...update } }));
  }

  function answerAsk(item: PendingItem): void {
    const draft = pendingDrafts[item.id] || {};
    answerPending(item, {
      answers: item.questions.map((question) => {
        const selectedOptions = draft.selectedOptions?.[question.id] || [];
        const customInput = draft.customInput?.[question.id] || "";
        return {
          id: question.id,
          selectedOptions,
          ...(customInput ? { customInput } : {}),
        };
      }),
    });
  }

  function toggleQuestionOption(item: PendingItem, question: PendingQuestion, option: string): void {
    const current = pendingDrafts[item.id]?.selectedOptions?.[question.id] || [];
    const next = question.multiple
      ? current.includes(option) ? current.filter((value) => value !== option) : [...current, option]
      : [option];
    setDraft(item.id, { selectedOptions: { ...pendingDrafts[item.id]?.selectedOptions, [question.id]: next } });
  }

  const statusIcon = {
    connecting: { label: "正在连接 core…", icon: LoaderCircle, className: "text-muted-foreground" },
    connected: { label: "core 已连接", icon: Check, className: "text-emerald-600" },
    disconnected: { label: "core 未连接", icon: CircleAlert, className: "text-destructive" },
  }[connection];
  const StatusIcon = statusIcon.icon;
  const currentProject = projects.find((project) => project.id === projectId);
  const controlsDisabled = Boolean(activeRunId) || connection !== "connected";

  return (
    <main className="flex h-screen min-h-0 flex-col bg-background text-foreground">
      <header className="flex shrink-0 items-center justify-between border-b px-6 py-3">
        <div className="flex items-center gap-3">
          {sessionId ? (
            <Button variant="ghost" size="sm" onClick={() => activateSession("", false)} aria-label="返回会话列表">
              <ArrowLeft className="size-4" /> 返回
            </Button>
          ) : null}
          <div>
            <p className="text-sm font-semibold">Lila Agent 工作台</p>
            <p className="text-xs text-muted-foreground">{currentProject?.path || "选择一个项目开始"}</p>
          </div>
        </div>
        <div className={`flex items-center gap-2 text-xs ${statusIcon.className}`}>
          <StatusIcon className={connection === "connecting" ? "size-4 animate-spin" : "size-4"} />
          {statusIcon.label}
          <span className="ml-2 text-muted-foreground">{status}</span>
        </div>
      </header>

      {!sessionId ? (
        <section className="mx-auto flex w-full max-w-6xl min-h-0 flex-1 gap-6 overflow-y-auto p-6">
          <div className="min-w-0 flex-1 rounded-xl border bg-card p-6">
            <h1 className="text-xl font-semibold">新建工作会话</h1>
            <p className="mt-2 text-sm text-muted-foreground">项目目录会作为 Agent 的工作目录。</p>
            <div className="mt-6 space-y-4">
              <label className="block text-sm font-medium">
                项目
                <select className="mt-2 w-full rounded-md border bg-background px-3 py-2 text-sm" value={projectId} onChange={(event) => setProjectId(event.target.value)} disabled={!projects.length}>
                  {!projects.length ? <option value="">暂无项目</option> : null}
                  {projects.map((project) => <option key={project.id} value={project.id}>{project.path}</option>)}
                </select>
              </label>
              <form className="flex gap-2" onSubmit={(event) => { event.preventDefault(); addProject(); }}>
                <label className="min-w-0 flex-1 text-sm font-medium">
                  添加绝对目录
                  <input className="mt-2 w-full rounded-md border bg-background px-3 py-2 text-sm" value={newProjectPath} onChange={(event) => setNewProjectPath(event.target.value)} placeholder="/Users/me/project" aria-label="绝对目录路径" />
                </label>
                <Button type="submit" variant="outline" className="mt-6" disabled={connection !== "connected" || !newProjectPath.trim()}><Plus className="size-4" />添加</Button>
              </form>
              <label className="block text-sm font-medium">
                模型
                <select className="mt-2 w-full rounded-md border bg-background px-3 py-2 text-sm" value={modelId} onChange={(event) => setModelId(event.target.value)} disabled={!models.length}>
                  <option value="">运行时默认模型</option>
                  {models.map((model) => <option key={model.id} value={model.id}>{model.label} ({model.id})</option>)}
                </select>
              </label>
              <label className="block text-sm font-medium">
                工具审批
                <select className="mt-2 w-full rounded-md border bg-background px-3 py-2 text-sm" value={approvalMode} onChange={(event) => setApprovalMode(event.target.value as ApprovalMode)}>
                  <option value="yolo">yolo：自动执行</option>
                  <option value="write">write：执行命令时询问</option>
                  <option value="always-ask">always-ask：写入和执行都询问</option>
                </select>
              </label>
              <Button disabled={connection !== "connected" || !projectId} onClick={createSession}>新建会话</Button>
            </div>
            {error ? <p className="mt-4 rounded-md bg-destructive/10 p-3 text-sm text-destructive">{error}</p> : null}
          </div>
          <aside className="w-80 shrink-0 rounded-xl border bg-card p-6">
            <h2 className="font-semibold">历史会话</h2>
            <div className="mt-4 space-y-2">
              {sessions.length ? sessions.map((session) => (
                <button key={session.id} type="button" className="w-full rounded-md border p-3 text-left hover:bg-muted" onClick={() => selectSession(session)}>
                  <p className="truncate font-mono text-xs">{session.id}</p>
                  <p className="mt-1 text-xs text-muted-foreground">{projects.find((project) => project.id === session.projectId)?.path || session.projectId}</p>
                  <p className="mt-1 text-xs text-muted-foreground">{session.status}</p>
                </button>
              )) : <p className="text-sm text-muted-foreground">暂无历史会话</p>}
            </div>
          </aside>
        </section>
      ) : (
        <section className="mx-auto flex w-full max-w-6xl min-h-0 flex-1 gap-4 p-4">
          <aside className="hidden w-64 shrink-0 overflow-y-auto rounded-xl border bg-card p-4 md:block">
            <div className="flex items-center justify-between"><h2 className="font-semibold">会话</h2><Button variant="ghost" size="sm" onClick={() => activateSession("", false)}><X className="size-4" /></Button></div>
            <div className="mt-3 space-y-2">
              {sessions.map((session) => <button key={session.id} type="button" className={`w-full rounded-md border p-2 text-left text-xs ${session.id === sessionId ? "bg-muted" : "hover:bg-muted"}`} onClick={() => selectSession(session)}><span className="block truncate font-mono">{session.id}</span><span className="text-muted-foreground">{session.status}</span></button>)}
            </div>
          </aside>
          <div className="flex min-w-0 flex-1 flex-col gap-3">
            <div className="flex shrink-0 flex-wrap items-center gap-3 rounded-xl border bg-card px-4 py-3">
              <span className="truncate font-mono text-xs text-muted-foreground">{sessionId}</span>
              <label className="ml-auto text-xs text-muted-foreground">模型 <select className="ml-1 rounded border bg-background px-2 py-1 text-xs" value={modelId} disabled={controlsDisabled} onChange={(event) => { const value = event.target.value; setModelId(value); configureSession(value, approvalMode); }}><option value="">运行时默认</option>{models.map((model) => <option key={model.id} value={model.id}>{model.label}</option>)}</select></label>
              <label className="text-xs text-muted-foreground">审批 <select className="ml-1 rounded border bg-background px-2 py-1 text-xs" value={approvalMode} disabled={controlsDisabled} onChange={(event) => { const value = event.target.value as ApprovalMode; setApprovalMode(value); configureSession(modelId, value); }}><option value="yolo">yolo</option><option value="write">write</option><option value="always-ask">always-ask</option></select></label>
            </div>
            <div className="min-h-0 flex-1 space-y-3 overflow-y-auto rounded-xl border bg-card p-4">
              {entries.length === 0 ? <p className="py-16 text-center text-sm text-muted-foreground">发送第一条消息开始工作。</p> : entries.map((entry) => {
                if (entry.kind === "user") return <article key={entry.id} className="ml-8 rounded-lg bg-primary p-3 text-sm text-primary-foreground"><p className="mb-1 text-xs font-medium opacity-70">你</p><pre className="whitespace-pre-wrap font-sans">{entry.text}</pre></article>;
                if (entry.kind === "assistant") return <article key={entry.id} className="mr-8 rounded-lg border p-3"><p className="mb-1 text-xs font-medium text-muted-foreground">assistant · {entry.messageId}</p>{entry.thinking ? <details className="mb-2 rounded bg-muted p-2 text-xs" open={!entry.completed}><summary className="cursor-pointer text-muted-foreground">thinking</summary><pre className="mt-2 whitespace-pre-wrap font-sans">{entry.thinking}</pre></details> : null}<pre className="whitespace-pre-wrap font-sans text-sm">{entry.text || (entry.completed ? "" : "正在生成…")}</pre></article>;
                return <article key={entry.id} className="rounded-lg border border-dashed bg-muted/40 p-3 text-sm"><div className="flex items-center justify-between"><p className="font-medium">工具：{entry.toolName}</p>{entry.isError ? <span className="text-xs text-destructive">失败</span> : null}</div>{entry.intent ? <p className="mt-1 text-xs text-muted-foreground">{entry.intent}</p> : null}<details className="mt-2"><summary className="cursor-pointer text-xs text-muted-foreground">参数</summary><pre className="mt-1 max-h-48 overflow-auto whitespace-pre-wrap text-xs">{asText(entry.args)}</pre></details>{entry.output ? <div className="mt-2"><p className="text-xs font-medium text-muted-foreground">增量 / 结果</p><pre className="mt-1 max-h-64 overflow-auto whitespace-pre-wrap text-xs">{entry.result || entry.output}</pre></div> : null}{entry.files ? <div className="mt-2"><p className="text-xs font-medium text-muted-foreground">文件 diff</p><pre className="mt-1 max-h-72 overflow-auto whitespace-pre-wrap text-xs">{asText(entry.files)}</pre></div> : null}</article>;
              })}
            </div>
            {pending.length ? <div className="max-h-[40vh] shrink-0 space-y-3 overflow-y-auto">{pending.map((item) => {
              const draft = pendingDrafts[item.id] || {};
              if (item.method === "cancel") return <section key={item.id} className="rounded-xl border border-amber-300 bg-amber-50 p-4 text-sm"><p>{item.title}</p><Button className="mt-3" variant="outline" onClick={() => answerPending(item, { cancelled: true })}>关闭</Button></section>;
              if (item.method === "confirm") return <section key={item.id} className="rounded-xl border border-amber-300 bg-amber-50 p-4 text-sm"><p className="font-medium">{item.title}</p>{item.message ? <p className="mt-1">{item.message}</p> : null}<div className="mt-3 flex gap-2"><Button onClick={() => answerPending(item, { confirmed: true })}>确认</Button><Button variant="outline" onClick={() => answerPending(item, { cancelled: true })}>取消</Button></div></section>;
              if (item.method === "select") return <section key={item.id} className="rounded-xl border border-amber-300 bg-amber-50 p-4 text-sm"><p className="font-medium">{item.title}</p><div className="mt-3 flex flex-wrap gap-2">{item.options.map((option) => <Button key={option} variant="outline" onClick={() => answerPending(item, { value: option })}>{option}</Button>)}<Button variant="ghost" onClick={() => answerPending(item, { cancelled: true })}>取消</Button></div></section>;
              if (item.method === "ask") return <section key={item.id} className="rounded-xl border border-amber-300 bg-amber-50 p-4 text-sm"><p className="font-medium">{item.title}</p>{item.questions.map((question) => <div key={question.id} className="mt-3"><p className="font-medium">{question.question}</p><div className="mt-2 flex flex-wrap gap-2">{question.options.map((option) => <Button key={option} type="button" variant={draft.selectedOptions?.[question.id]?.includes(option) ? "default" : "outline"} onClick={() => toggleQuestionOption(item, question, option)}>{option}</Button>)}</div><input className="mt-2 w-full rounded border bg-background px-2 py-1 text-sm" value={draft.customInput?.[question.id] || ""} onChange={(event) => setDraft(item.id, { customInput: { ...draft.customInput, [question.id]: event.target.value } })} placeholder="自定义回答（可选）" /></div>)}<div className="mt-3 flex gap-2"><Button onClick={() => answerAsk(item)}>提交回答</Button><Button variant="outline" onClick={() => answerPending(item, { cancelled: true })}>取消</Button></div></section>;
              return <section key={item.id} className="rounded-xl border border-amber-300 bg-amber-50 p-4 text-sm"><label className="font-medium">{item.title}<textarea className="mt-2 min-h-20 w-full rounded border bg-background p-2" value={draft.value ?? item.defaultValue ?? ""} onChange={(event) => setDraft(item.id, { value: event.target.value })} placeholder={item.placeholder} /></label><div className="mt-3 flex gap-2"><Button onClick={() => answerPending(item, { value: draft.value ?? item.defaultValue ?? "" })}>提交</Button><Button variant="outline" onClick={() => answerPending(item, { cancelled: true })}>取消</Button></div></section>;
            })}</div> : null}
            {error ? <p className="shrink-0 rounded-md bg-destructive/10 p-2 text-sm text-destructive">{error}</p> : null}
            <div className="flex shrink-0 items-end gap-3 rounded-xl border bg-card p-3"><label className="sr-only" htmlFor="composer">消息</label><textarea id="composer" className="min-h-16 flex-1 resize-none bg-transparent p-2 text-sm outline-none" value={composer} onChange={(event) => setComposer(event.target.value)} onKeyDown={(event) => { if (event.key === "Enter" && !event.shiftKey) { event.preventDefault(); sendMessage(); } }} placeholder="描述你要完成的编程任务…" disabled={controlsDisabled} />{activeRunId ? <Button variant="outline" onClick={abortRun}><Square className="size-4" />停止</Button> : <Button onClick={sendMessage} disabled={controlsDisabled || !composer.trim()}><Send className="size-4" />发送</Button>}</div>
            <p className="shrink-0 text-xs text-muted-foreground">事件序号：{lastEventSeq} · approval mode：{approvalMode}</p>
          </div>
        </section>
      )}
    </main>
  );
}

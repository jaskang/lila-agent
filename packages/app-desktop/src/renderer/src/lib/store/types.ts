import type {
  ApprovalMode,
  AuthLoginPrompt,
  AuthMethod,
  ImageInput,
  ModelInfo,
  Project,
  SessionState,
  SessionSummary,
  ThinkingLevel,
  TranscriptItem,
} from "@lila/protocol";
import type { ConnectionState } from "../client.ts";
import type { FileChange } from "../transcript-layout.ts";

export type SessionView = {
  items: TranscriptItem[];
  /** Item id → index in `items`. */
  index: Record<string, number>;
  hasEarlier: boolean;
  loadingEarlier: boolean;
  state?: SessionState;
  loading: boolean;
  error?: string;
};

/** Settings for a session that does not exist yet; remembered between new sessions. */
export type NewSessionOptions = { projectId?: string; modelId?: string; thinkingLevel?: ThinkingLevel };
export type DraftImage = ImageInput & { id: string; name: string };
export type Draft = { text: string; images: DraftImage[] };
export type DeliveryMode = "steer" | "followUp";

export type RightPanelKind = "files" | "changes" | "subsession" | "review";
export type FileHistoryEntry = { path: string; line?: number | undefined; endLine?: number | undefined; scrollTop?: number | undefined };
export type ReplyReviewState = { sessionId: string; changes: FileChange[]; path: string };
export type SettingsTab = "general" | "providers" | "usage" | "mcp" | "memory" | "capabilities" | "about";
export type AuthLoginFlowState = {
  loginId: string;
  providerId: string;
  providerName: string;
  method: AuthMethod;
  status: "running" | "done" | "error";
  authUrl?: { url: string; instructions?: string };
  deviceCode?: { userCode: string; verificationUri: string };
  info?: { message: string; links?: Array<{ url: string; label?: string }> };
  progress?: string;
  prompt?: AuthLoginPrompt;
  error?: string;
};

export type State = {
  connection: ConnectionState;
  connectionDetail?: string;
  projects: Project[];
  models: ModelInfo[];
  defaultModelId?: string;
  modelsError?: string;
  modelsLoading: boolean;
  sessions: Record<string, SessionSummary>;
  /** Parent session id → child sub-agent sessions, sorted by newest first. */
  childrenByParent: Record<string, SessionSummary[]>;
  views: Record<string, SessionView>;
  selectedId?: string;
  previewId?: string;
  /** When user navigates from a sub-session preview into files/review, remembers which sub-session to return to. */
  returnToPreviewId?: string;
  newSession: NewSessionOptions;
  defaultApprovalMode: ApprovalMode;
  newSessionApprovalMode?: ApprovalMode | undefined;
  /** Composer text per session; `new` is the new-session composer. */
  drafts: Record<string, string>;
  /** Pending image attachments per session; preserved across session switches and refreshes. */
  draftImages: Record<string, DraftImage[]>;
  sendingDrafts: Record<string, Draft>;
  paletteOpen: boolean;
  settingsOpen: boolean;
  settingsTab: SettingsTab;
  settingsDraftStatus?: "dirty" | "saving" | undefined;
  confirmSettingsDiscard: boolean;
  settingsDirty: { mcp: boolean; memory: boolean; capabilities: boolean };
  notificationsEnabled: boolean;
  authLogin?: AuthLoginFlowState | undefined;
  rightPanel?: RightPanelKind | undefined;
  activeFile?: { path: string; line?: number | undefined; endLine?: number | undefined; scrollTop?: number | undefined; history: FileHistoryEntry[] } | undefined;
  activeChangePath?: string | undefined;
  replyReview?: ReplyReviewState | undefined;
  gitChangeCounts: Record<string, number | undefined>;
  /** Background sessions that finished while unselected, awaiting user review. */
  unreadCompletedIds: Record<string, true>;
  workspaceRefreshSeq: number;
  commandsRefreshSeq: number;
};

export const NEW_DRAFT = "new";
export const SELECTED_KEY = "lila.selectedSession";
export const NEW_SESSION_KEY = "lila.newSession";
export const DEFAULT_APPROVAL_KEY = "lila.defaultApprovalMode";
export const NOTIFICATIONS_KEY = "lila.notificationsEnabled";
export const DRAFTS_SESSION_KEY = "lila.drafts";
export const OPEN_LIMIT = 300;
export const EARLIER_LIMIT = 200;
export const MAX_CACHED_VIEWS = 10;
export const MAX_SCROLL_POSITIONS = 50;

export const APPROVAL_LABEL: Record<ApprovalMode, string> = {
  "ask-all": "请求批准",
  "ask-exec": "帮我批准",
  auto: "完全访问权限",
};

export const EMPTY_DRAFT: Draft = { text: "", images: [] };

export const EMPTY_VIEW: SessionView = Object.freeze({
  items: [],
  index: {},
  hasEarlier: false,
  loadingEarlier: false,
  loading: false,
});

export const emptyView = (): SessionView => ({ items: [], index: {}, hasEarlier: false, loadingEarlier: false, loading: false });

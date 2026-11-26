import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { ApprovalMode, DEFAULT_APPROVAL_MODE, type Project } from "@lila/protocol";

type State = {
  projects: Project[];
  approvalModes: Record<string, ApprovalMode>;
  approvalRules: Record<string, string[]>;
};

/** Lila's own state: projects, per-session approval modes, and per-project reusable approval rules. */
export class Store {
  readonly #path: string;
  #state: State;

  constructor(dataDir: string) {
    mkdirSync(dataDir, { recursive: true, mode: 0o700 });
    this.#path = join(dataDir, "state.json");
    const loaded = this.#load();
    this.#state = {
      projects: Array.isArray(loaded.projects) ? loaded.projects : [],
      approvalModes: loaded.approvalModes && typeof loaded.approvalModes === "object" ? loaded.approvalModes : {},
      approvalRules: loaded.approvalRules && typeof loaded.approvalRules === "object" ? loaded.approvalRules : {},
    };
  }

  #load(): Partial<State> {
    if (!existsSync(this.#path)) return {};
    try {
      const parsed = JSON.parse(readFileSync(this.#path, "utf8")) as unknown;
      if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) return parsed as Partial<State>;
      throw new Error("state.json root is not an object");
    } catch (error) {
      const backup = `${this.#path}.corrupt.${Date.now()}`;
      try {
        renameSync(this.#path, backup);
        console.warn(`[core] corrupted state.json backed up to ${backup}`, error);
      } catch (backupError) {
        console.warn(`[core] failed to back up corrupted state.json`, backupError);
      }
      return {};
    }
  }

  get projects(): readonly Project[] {
    return this.#state.projects;
  }

  project(id: string): Project | undefined {
    return this.#state.projects.find((project) => project.id === id);
  }

  addProject(project: Project): void {
    this.#state.projects.push(project);
    this.#save();
  }

  removeProject(id: string): void {
    this.#state.projects = this.#state.projects.filter((project) => project.id !== id);
    delete this.#state.approvalRules[id];
    this.#save();
  }

  approvalRules(projectId: string): string[] {
    const list = this.#state.approvalRules[projectId];
    return Array.isArray(list) ? list.filter((item): item is string => typeof item === "string" && item.length > 0) : [];
  }

  addApprovalRule(projectId: string, pattern: string): void {
    const current = this.approvalRules(projectId);
    if (current.includes(pattern)) return;
    this.#state.approvalRules[projectId] = [...current, pattern];
    this.#save();
  }

  removeApprovalRule(projectId: string, pattern: string): void {
    const current = this.approvalRules(projectId);
    const next = current.filter((item) => item !== pattern);
    if (next.length === current.length) return;
    if (next.length) this.#state.approvalRules[projectId] = next;
    else delete this.#state.approvalRules[projectId];
    this.#save();
  }

  clearApprovalRules(projectId: string): void {
    if (!(projectId in this.#state.approvalRules)) return;
    delete this.#state.approvalRules[projectId];
    this.#save();
  }

  approvalMode(sessionId: string): ApprovalMode {
    const saved = ApprovalMode.safeParse(this.#state.approvalModes[sessionId]);
    return saved.success ? saved.data : DEFAULT_APPROVAL_MODE;
  }

  setApprovalMode(sessionId: string, mode: ApprovalMode): void {
    this.#state.approvalModes[sessionId] = mode;
    this.#save();
  }

  forgetSession(sessionId: string): void {
    if (!(sessionId in this.#state.approvalModes)) return;
    delete this.#state.approvalModes[sessionId];
    this.#save();
  }

  /** Atomic: a crash mid-write leaves the previous file intact. */
  #save(): void {
    const tmp = `${this.#path}.${process.pid}.tmp`;
    writeFileSync(tmp, `${JSON.stringify(this.#state, null, 2)}\n`, { mode: 0o600 });
    renameSync(tmp, this.#path);
  }
}

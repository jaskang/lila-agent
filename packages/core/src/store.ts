import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { ApprovalMode, Project } from "@lila/protocol";

type State = { projects: Project[]; approvalModes: Record<string, ApprovalMode> };

/** Lila's own state: projects and per-session approval modes. Transcripts live in Pi's session files. */
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
    this.#save();
  }

  approvalMode(sessionId: string): ApprovalMode {
    return this.#state.approvalModes[sessionId] ?? "auto";
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

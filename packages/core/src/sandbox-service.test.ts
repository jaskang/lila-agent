import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, describe, expect, test } from "vitest";
import { createSandboxedBashOperations, resolveWorktreeGitCommonDir, SandboxService } from "./sandbox-service.ts";
import { SchedulerService } from "./scheduler-service.ts";
import { Store } from "./store.ts";

describe("SandboxService", () => {
  const root = mkdtempSync(join(tmpdir(), "lila-sandbox-test-"));
  const dataDir = join(root, "data");
  const projA = join(root, "proj-a");
  const projB = join(root, "proj-b");
  const wtB = join(root, "proj-b-wt");
  mkdirSync(dataDir, { recursive: true });
  mkdirSync(projA, { recursive: true });
  mkdirSync(projB, { recursive: true });
  writeFileSync(join(dataDir, "core-token"), "secret-core-token");
  writeFileSync(join(projA, ".env"), "SECRET=1");

  execFileSync("git", ["init", "-b", "main"], { cwd: projB, stdio: "ignore" });
  execFileSync("git", ["config", "user.email", "test@lila.local"], { cwd: projB, stdio: "ignore" });
  execFileSync("git", ["config", "user.name", "Lila Test"], { cwd: projB, stdio: "ignore" });
  writeFileSync(join(projB, "README.md"), "hello\n");
  execFileSync("git", ["add", "."], { cwd: projB, stdio: "ignore" });
  execFileSync("git", ["commit", "-m", "init"], { cwd: projB, stdio: "ignore" });
  execFileSync("git", ["worktree", "add", "-b", "wt-branch", wtB], { cwd: projB, stdio: "ignore" });

  const sandbox = new SandboxService(dataDir);

  afterAll(async () => {
    await sandbox.dispose();
    rmSync(root, { recursive: true, force: true });
  });

  test("resolves shared .git common dir for worktrees and ignores regular repos", () => {
    expect(resolveWorktreeGitCommonDir(projB)).toBeUndefined();
    expect(resolveWorktreeGitCommonDir(wtB)).toBe(realpathSync(join(projB, ".git")));
  });

  test("persists sandbox mode per session and defaults schedules to workspace-write", () => {
    const store = new Store(dataDir);
    expect(store.sandboxMode("s1")).toBe("off");
    store.setSandboxMode("s1", "workspace-write");
    expect(new Store(dataDir).sandboxMode("s1")).toBe("workspace-write");
    store.forgetSession("s1");
    expect(new Store(dataDir).sandboxMode("s1")).toBe("off");

    const scheduler = new SchedulerService(dataDir, {
      hasProject: () => true,
      executeJob: async () => ({}),
      onChange: () => {},
    });
    const job = scheduler.create({
      name: "nightly",
      projectId: "p1",
      schedule: "1h",
      prompt: "check status",
    });
    expect(job.sandboxMode).toBe("workspace-write");
    scheduler.dispose();
  });

  const canRunRealSandbox = sandbox.status(false).available;

  test.runIf(canRunRealSandbox)("isolates concurrent projects, blocks secret reads and .env/.git/hooks writes", async () => {
    const runIn = async (projectRoot: string, command: string) => {
      const ops = createSandboxedBashOperations({
        sandbox,
        getSandboxMode: () => "workspace-write",
        projectRoot,
      });
      let output = "";
      const { exitCode } = await ops.exec(command, projectRoot, {
        onData: (chunk) => {
          output += chunk.toString("utf8");
        },
      });
      return { exitCode, output };
    };

    const [aWriteA, aWriteB, bWriteA, readToken, writeEnv, writeHook, wtCommit] = await Promise.all([
      runIn(projA, `echo ok > "${join(projA, "ok.txt")}"`),
      runIn(projA, `echo bad > "${join(projB, "bad.txt")}"`),
      runIn(projB, `echo bad > "${join(projA, "bad.txt")}"`),
      runIn(projA, `cat "${join(dataDir, "core-token")}"`),
      runIn(projA, `echo hack > "${join(projA, ".env")}"`),
      runIn(projB, `echo hack > "${join(projB, ".git", "hooks", "pre-commit")}"`),
      runIn(wtB, `echo wt > wt.txt && git add wt.txt && git commit -m "wt commit"`),
    ]);

    expect(aWriteA.exitCode).toBe(0);
    expect(aWriteB.exitCode).not.toBe(0);
    expect(bWriteA.exitCode).not.toBe(0);
    expect(readToken.exitCode).not.toBe(0);
    expect(readToken.output).not.toContain("secret-core-token");
    expect(writeEnv.exitCode).not.toBe(0);
    expect(writeHook.exitCode).not.toBe(0);
    expect(wtCommit.exitCode).toBe(0);
  });
});

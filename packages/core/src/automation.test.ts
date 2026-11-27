import { expect, test } from "vitest";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { parseScheduleExpression, SchedulerService } from "./scheduler-service.ts";
import { buildTaskExecutionPrompt, inferTaskLinkKind, TaskService } from "./task-service.ts";

test("parses cron, interval, and one-shot expressions and rejects unsafe/past inputs", () => {
  const now = Date.parse("2026-11-27T09:00:00.000Z");

  const cron5 = parseScheduleExpression("0 9 * * 1-5", now);
  expect(cron5.kind).toBe("cron");
  expect(cron5.nextRunAt).toBeTruthy();

  const cron6 = parseScheduleExpression("30 0 9 * * *", now);
  expect(cron6.kind).toBe("cron");

  const interval = parseScheduleExpression("15m", now);
  expect(interval).toMatchObject({ kind: "interval", normalized: "15m", intervalMs: 15 * 60_000 });

  const relOnce = parseScheduleExpression("+30m", now);
  expect(relOnce.kind).toBe("once");
  expect(relOnce.normalized).toBe("2026-11-27T09:30:00.000Z");

  expect(() => parseScheduleExpression("5s", now)).toThrow("至少需要 10 秒");
  expect(() => parseScheduleExpression("2020-01-01T00:00:00.000Z", now)).toThrow("已过");
  expect(() => parseScheduleExpression("invalid cron", now)).toThrow("无法识别");
});

test("persists scheduled jobs across restarts, runs manually, and cleans up on project removal", async () => {
  const dir = mkdtempSync(join(tmpdir(), "lila-sched-"));
  const executed: string[] = [];
  const projects = new Set(["proj-1"]);

  const scheduler1 = new SchedulerService(dir, {
    hasProject: (id) => projects.has(id),
    executeJob: async (job) => {
      executed.push(job.name);
      return { sessionId: "sess-1" };
    },
    onChange: () => {},
  });

  try {
    const job = scheduler1.create({
      name: "每日巡检",
      projectId: "proj-1",
      schedule: "0 9 * * 1-5",
      prompt: "检查未解决的问题",
      sessionMode: "reuse",
    });
    expect(job.enabled).toBe(true);
    expect(job.nextRunAt).toBeTruthy();

    const ran = await scheduler1.runNow(job.id);
    expect(executed).toEqual(["每日巡检"]);
    expect(ran.sessionId).toBe("sess-1");
    expect(ran.schedule.runCount).toBe(1);
    expect(ran.schedule.lastStatus).toBe("success");
    scheduler1.dispose();

    const scheduler2 = new SchedulerService(dir, {
      hasProject: (id) => projects.has(id),
      executeJob: async () => ({ sessionId: "sess-2" }),
      onChange: () => {},
    });
    try {
      const reloaded = scheduler2.list();
      expect(reloaded).toHaveLength(1);
      expect(reloaded[0]?.name).toBe("每日巡检");
      expect(reloaded[0]?.runCount).toBe(1);

      projects.delete("proj-1");
      scheduler2.removeByProject("proj-1");
      expect(scheduler2.list()).toEqual([]);
    } finally {
      scheduler2.dispose();
    }
  } finally {
    scheduler1.dispose();
    rmSync(dir, { recursive: true, force: true });
  }
});

test("extracts design/issue links, transitions running tasks to review, and builds design-aware prompt", () => {
  const dir = mkdtempSync(join(tmpdir(), "lila-tasks-"));
  try {
    expect(inferTaskLinkKind("https://design.example.com/checkout/index.html")).toBe("design");
    expect(inferTaskLinkKind("https://github.com/org/repo/issues/42")).toBe("issue");
    expect(inferTaskLinkKind("https://github.com/org/repo/pull/99")).toBe("pr");
    expect(inferTaskLinkKind("https://app.asana.com/0/123/456")).toBe("issue");

    const service = new TaskService(dir, () => {});
    const created = service.create({
      title: "实现结算页改版",
      projectId: "proj-1",
      source: "wechat",
      summary: "设计师刚发了新稿 https://preview.example.com/checkout.html 以及需求 https://app.asana.com/0/123/456",
    });
    expect(created.links).toEqual([
      { url: "https://preview.example.com/checkout.html", kind: "design" },
      { url: "https://app.asana.com/0/123/456", kind: "issue" },
    ]);

    const prompt = buildTaskExecutionPrompt(created);
    expect(prompt).toContain("[设计稿]");
    expect(prompt).toContain("严格映射到当前仓库的组件与样式体系实现");

    service.update(created.id, { status: "running", sessionId: "sess-task-1" });
    service.onSessionTransition("sess-task-1", "running", "idle");
    expect(service.get(created.id)?.status).toBe("review");

    // Persistence check across restart.
    const reloaded = new TaskService(dir, () => {});
    expect(reloaded.get(created.id)?.status).toBe("review");
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

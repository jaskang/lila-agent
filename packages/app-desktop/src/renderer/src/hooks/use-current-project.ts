import { useMemo } from "react";
import { useLila } from "@/lib/store";

export function useNewSessionProjectId(): string | undefined {
  const projects = useLila((state) => state.projects);
  const sessions = useLila((state) => state.sessions);
  const projectId = useLila((state) => state.newSession.projectId);

  return useMemo(() => {
    if (projects.some((project) => project.id === projectId)) return projectId;
    const projectIds = new Set(projects.map((project) => project.id));
    let recent: { projectId: string; updatedAt: string } | undefined;
    for (const session of Object.values(sessions)) {
      if (session.parentId || !projectIds.has(session.projectId)) continue;
      if (!recent || session.updatedAt > recent.updatedAt) recent = session;
    }
    return recent?.projectId ?? projects.at(-1)?.id;
  }, [projectId, projects, sessions]);
}

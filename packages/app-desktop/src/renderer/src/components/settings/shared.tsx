import * as React from "react";
import { ExternalLink, FolderOpen } from "lucide-react";
import { toast } from "sonner";
import type { ModelInfo } from "@lila/protocol";
import { IconButton } from "@/components/icon-button";
import { actions } from "@/lib/store";

export function SettingsSection({
  title,
  action,
  children,
}: {
  title?: string;
  action?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section className="flex flex-col gap-1.5">
      {title || action ? (
        <div className="flex items-center justify-between gap-2 px-1">
          {title ? <h3 className="text-xs font-medium text-muted-foreground">{title}</h3> : <span />}
          {action}
        </div>
      ) : null}
      <div className="divide-y divide-border/60 border-y border-border/60">{children}</div>
    </section>
  );
}

export function SettingsRow({
  title,
  detail,
  children,
}: {
  title: string;
  detail?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-center justify-between gap-4 px-1 py-2.5">
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="truncate text-sm text-foreground">{title}</span>
        {detail ? <span className="truncate text-xs text-muted-foreground">{detail}</span> : null}
      </div>
      <div className="shrink-0">{children}</div>
    </div>
  );
}

export function PathRow({ title, path }: { title: string; path: string }) {
  return (
    <div className="flex items-center justify-between gap-3 px-1 py-2.5">
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="text-sm text-foreground">{title}</span>
        <span className="truncate font-mono text-[11px] text-muted-foreground" title={path}>
          {path}
        </span>
      </div>
      <div className="flex shrink-0 items-center gap-0.5">
        <IconButton
          size="icon-xs"
          label="打开"
          onClick={() =>
            void window.lila
              .openPath(path)
              .catch((error: unknown) => toast.error(error instanceof Error ? error.message : String(error)))
          }
        >
          <ExternalLink />
        </IconButton>
        <IconButton size="icon-xs" label="在访达中显示" onClick={() => void actions.revealPath(path)}>
          <FolderOpen />
        </IconButton>
      </div>
    </div>
  );
}

export function useGroupedModels(models: ModelInfo[], pinnedModelId?: string) {
  const enabledModels = React.useMemo(() => {
    const enabled = models.filter((item) => item.enabled);
    const base = enabled.length ? enabled : models;
    if (pinnedModelId && !base.some((m) => m.id === pinnedModelId)) {
      const fallback = models.find((m) => m.id === pinnedModelId);
      if (fallback) return [fallback, ...base];
    }
    return base;
  }, [models, pinnedModelId]);

  const groupedModels = React.useMemo(() => {
    const groups = new Map<string, { providerName: string; items: ModelInfo[] }>();
    for (const model of enabledModels) {
      let group = groups.get(model.provider);
      if (!group) {
        group = { providerName: model.providerName, items: [] };
        groups.set(model.provider, group);
      }
      group.items.push(model);
    }
    return [...groups.entries()];
  }, [enabledModels]);

  return { enabledModels, groupedModels };
}

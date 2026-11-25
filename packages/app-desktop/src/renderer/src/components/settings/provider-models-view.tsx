import * as React from "react";
import { ArrowLeft, Check, RotateCw } from "lucide-react";
import { toast } from "sonner";
import type { ModelInfo } from "@lila/protocol";
import { SearchInput } from "@/components/search-input";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Empty, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Spinner } from "@/components/ui/spinner";
import { cn } from "cn";
import { actions, useLila } from "@/lib/store";

const COMPACT = new Intl.NumberFormat("en", { notation: "compact", maximumFractionDigits: 0 });

function rawModelId(fullId: string, providerId: string): string {
  const prefix = `${providerId}/`;
  return fullId.startsWith(prefix) ? fullId.slice(prefix.length) : fullId;
}

export function ProviderModelsView({
  providerId,
  providerName,
  models,
  onBack,
}: {
  providerId: string;
  providerName: string;
  models: ModelInfo[];
  onBack: () => void;
}) {
  const modelsLoading = useLila((state) => state.modelsLoading);
  const [selected, setSelected] = React.useState<Set<string>>(
    () => new Set(models.filter((item) => item.enabled).map((item) => item.id)),
  );
  const [query, setQuery] = React.useState("");
  const [saving, setSaving] = React.useState(false);
  const [discardOpen, setDiscardOpen] = React.useState(false);

  const isDirty = React.useMemo(() => {
    const baseline = models.filter((item) => item.enabled).map((item) => item.id);
    if (baseline.length !== selected.size) return true;
    return baseline.some((id) => !selected.has(id));
  }, [models, selected]);

  const requestBack = () => {
    if (isDirty) {
      setDiscardOpen(true);
      return;
    }
    onBack();
  };

  const prevLenRef = React.useRef(models.length);
  React.useEffect(() => {
    if (prevLenRef.current === 0 && models.length > 0) {
      setSelected(new Set(models.filter((item) => item.enabled).map((item) => item.id)));
    }
    prevLenRef.current = models.length;
  }, [models]);

  const filtered = React.useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return models;
    return models.filter(
      (model) => model.label.toLowerCase().includes(needle) || model.id.toLowerCase().includes(needle),
    );
  }, [models, query]);

  const toggle = (id: string) => {
    setSelected((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const handleSave = async () => {
    if (saving || selected.size === 0) return;
    setSaving(true);
    try {
      const allSelected = models.length > 0 && selected.size === models.length;
      const payload = allSelected ? null : [...selected].map((fullId) => rawModelId(fullId, providerId));
      if (await actions.setProviderModels(providerId, payload)) {
        toast.success(allSelected ? `已启用全部 ${models.length} 个模型` : `已保存 ${selected.size} 个模型`);
        onBack();
      }
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="flex flex-col gap-4">
      <div className="flex items-center justify-between gap-2">
        <div className="flex min-w-0 items-center gap-2">
          <Button type="button" size="sm" variant="ghost" className="-ms-2 text-muted-foreground hover:text-foreground" onClick={requestBack}>
            <ArrowLeft />
            返回
          </Button>
          <span className="truncate text-sm font-medium text-foreground">{providerName}</span>
        </div>
        <span className="shrink-0 font-mono text-xs text-muted-foreground tabular-nums">
          {selected.size} / {models.length}
        </span>
      </div>

      <div className="flex items-center gap-2">
        <SearchInput value={query} onChange={setQuery} placeholder="搜索模型" ariaLabel="搜索模型" />
        <Button type="button" size="xs" variant="ghost" disabled={saving || selected.size === models.length} onClick={() => setSelected(new Set(models.map((item) => item.id)))}>
          全选
        </Button>
        <Button type="button" size="xs" variant="ghost" disabled={saving || selected.size === 0} onClick={() => setSelected(new Set())}>
          清空
        </Button>
      </div>

      {!models.length ? (
        <Empty className="py-10">
          <EmptyHeader>
            <EmptyTitle>{modelsLoading ? "正在读取模型…" : "无可用模型"}</EmptyTitle>
          </EmptyHeader>
          <Button type="button" size="sm" variant="outline" disabled={modelsLoading} onClick={() => void actions.retryModels()}>
            {modelsLoading ? <Spinner /> : <RotateCw />}
            刷新
          </Button>
        </Empty>
      ) : !filtered.length ? (
        <Empty className="py-8">
          <EmptyHeader>
            <EmptyTitle>无匹配模型</EmptyTitle>
          </EmptyHeader>
        </Empty>
      ) : (
        <div className="divide-y divide-border/50 border-y border-border/60">
          {filtered.map((model) => {
            const checked = selected.has(model.id);
            const shortId = rawModelId(model.id, providerId);
            const meta = [
              model.contextWindow ? COMPACT.format(model.contextWindow) : undefined,
              model.reasoning ? "推理" : undefined,
              model.images ? "识图" : undefined,
            ]
              .filter(Boolean)
              .join(" · ");
            return (
              <button
                key={model.id}
                type="button"
                role="checkbox"
                aria-checked={checked}
                disabled={saving}
                onClick={() => toggle(model.id)}
                className="flex w-full items-center gap-3 px-1 py-2.5 text-left transition-colors hover:bg-muted/40 focus-visible:bg-muted/50 focus-visible:outline-none"
              >
                <span
                  aria-hidden
                  className={cn(
                    "flex size-4 shrink-0 items-center justify-center rounded-[4px] border transition-colors",
                    checked ? "border-primary bg-primary text-primary-foreground" : "border-input bg-background text-transparent",
                  )}
                >
                  <Check className="size-3 stroke-[2.5]" />
                </span>
                <div className="flex min-w-0 flex-1 flex-col gap-0.5">
                  <span className="truncate text-sm text-foreground">{model.label}</span>
                  {shortId !== model.label ? <span className="truncate font-mono text-[11px] text-muted-foreground">{shortId}</span> : null}
                </div>
                {meta ? <span className="shrink-0 font-mono text-[11px] text-muted-foreground tabular-nums">{meta}</span> : null}
              </button>
            );
          })}
        </div>
      )}

      <div className="flex items-center justify-end gap-2 pt-2">
        <Button type="button" size="sm" variant="outline" disabled={saving} onClick={requestBack}>
          取消
        </Button>
        <Button type="submit" size="sm" disabled={saving || selected.size === 0} onClick={() => void handleSave()}>
          {saving ? <Spinner /> : null}
          保存
        </Button>
      </div>

      <AlertDialog open={discardOpen} onOpenChange={setDiscardOpen}>
        <AlertDialogContent size="sm">
          <AlertDialogHeader>
            <AlertDialogTitle>放弃未保存的更改？</AlertDialogTitle>
            <AlertDialogDescription>返回后将丢弃对「{providerName}」的模型选择。</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>继续编辑</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={() => {
                setDiscardOpen(false);
                onBack();
              }}
            >
              放弃
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

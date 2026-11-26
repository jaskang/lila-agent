import * as React from "react";
import { Check, ChevronRight, ExternalLink, FolderOpen, Info, Pencil, Plus, RotateCw, Trash2, TriangleAlert, X } from "lucide-react";
import { toast } from "sonner";
import type { MemorySnapshot, MemoryStoreSnapshot, MemoryTarget } from "@lila/protocol";
import { IconButton } from "@/components/icon-button";
import { SettingsRow, SettingsSection, useGroupedModels } from "@/components/settings/shared";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
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
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { Input } from "@/components/ui/input";
import { Progress } from "@/components/ui/progress";
import { Select, SelectContent, SelectGroup, SelectItem, SelectLabel, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "cn";
import { actions, useLila } from "@/lib/store";

const AUTO_MODEL_VALUE = "__auto__";

function MemoryNotesList({
  store,
  busy,
  onAdd,
  onReplace,
  onRemove,
}: {
  store: MemoryStoreSnapshot;
  busy: boolean;
  onAdd: (content: string) => Promise<boolean>;
  onReplace: (oldText: string, newContent: string) => Promise<boolean>;
  onRemove: (oldText: string) => Promise<void>;
}) {
  const [composerOpen, setComposerOpen] = React.useState(false);
  const [draft, setDraft] = React.useState("");
  const [adding, setAdding] = React.useState(false);
  const [editingIndex, setEditingIndex] = React.useState<number>();
  const [editText, setEditText] = React.useState("");
  const [savingEdit, setSavingEdit] = React.useState(false);
  const [confirmDelete, setConfirmDelete] = React.useState<string>();
  const [deleting, setDeleting] = React.useState(false);
  const addInputRef = React.useRef<HTMLTextAreaElement>(null);

  React.useEffect(() => {
    setEditingIndex(undefined);
    setComposerOpen(false);
    setDraft("");
  }, [store.target]);

  React.useEffect(() => {
    if (composerOpen) addInputRef.current?.focus();
  }, [composerOpen]);

  const submitAdd = async () => {
    const trimmed = draft.trim();
    if (!trimmed || adding || busy) return;
    setAdding(true);
    try {
      if (await onAdd(trimmed)) {
        setDraft("");
        setComposerOpen(false);
      }
    } finally {
      setAdding(false);
    }
  };

  const submitEdit = async (original: string) => {
    const trimmed = editText.trim();
    if (!trimmed || savingEdit) return;
    if (trimmed === original) {
      setEditingIndex(undefined);
      return;
    }
    setSavingEdit(true);
    try {
      if (await onReplace(original, trimmed)) setEditingIndex(undefined);
    } finally {
      setSavingEdit(false);
    }
  };

  const confirmRemove = async () => {
    if (!confirmDelete || deleting) return;
    setDeleting(true);
    try {
      await onRemove(confirmDelete);
      setConfirmDelete(undefined);
    } finally {
      setDeleting(false);
    }
  };

  return (
    <>
      {store.entries.length ? (
        <div className="divide-y divide-border/50">
          {store.entries.map((entry, index) => {
            const editing = editingIndex === index;
            return (
              <div key={`${index}:${entry.slice(0, 32)}`} className="group/entry px-1 py-2.5 transition-colors hover:bg-muted/35">
                {editing ? (
                  <div className="flex flex-col gap-2">
                    <Textarea
                      value={editText}
                      disabled={savingEdit}
                      rows={3}
                      className="min-h-[4.5rem] resize-none bg-background text-xs leading-relaxed"
                      onChange={(event) => setEditText(event.target.value)}
                      onKeyDown={(event) => {
                        if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
                          event.preventDefault();
                          void submitEdit(entry);
                        } else if (event.key === "Escape") {
                          setEditingIndex(undefined);
                        }
                      }}
                    />
                    <div className="flex items-center justify-end gap-1.5">
                      <Button type="button" size="xs" variant="ghost" disabled={savingEdit} onClick={() => setEditingIndex(undefined)}>
                        <X />
                        取消
                      </Button>
                      <Button type="button" size="xs" disabled={savingEdit || !editText.trim()} onClick={() => void submitEdit(entry)}>
                        {savingEdit ? <Spinner /> : <Check />}
                        保存
                      </Button>
                    </div>
                  </div>
                ) : (
                  <div className="flex items-start gap-2.5">
                    <p className="min-w-0 flex-1 select-text whitespace-pre-wrap break-words text-[13px] leading-relaxed text-foreground">
                      {entry}
                    </p>
                    <div className="flex shrink-0 items-center gap-0.5 opacity-0 transition-opacity duration-150 group-hover/entry:opacity-100 focus-within:opacity-100 pointer-coarse:opacity-100">
                      <IconButton
                        size="icon-xs"
                        label="编辑"
                        disabled={busy}
                        onClick={() => {
                          setEditingIndex(index);
                          setEditText(entry);
                        }}
                      >
                        <Pencil />
                      </IconButton>
                      <IconButton
                        size="icon-xs"
                        label="删除"
                        disabled={busy}
                        className="hover:text-destructive"
                        onClick={() => setConfirmDelete(entry)}
                      >
                        <Trash2 />
                      </IconButton>
                    </div>
                  </div>
                )}
              </div>
            );
          })}
        </div>
      ) : (
        <p className="px-1 py-6 text-center text-xs text-muted-foreground">暂无条目</p>
      )}

      <div className="border-t border-border/60 pt-2.5">
        {composerOpen ? (
          <form
            onSubmit={(event) => {
              event.preventDefault();
              void submitAdd();
            }}
            className="flex flex-col gap-2"
          >
            <Textarea
              ref={addInputRef}
              value={draft}
              disabled={adding || busy}
              rows={2}
              placeholder="输入一条记忆…"
              className="min-h-[3.75rem] resize-none bg-background text-xs leading-relaxed"
              onChange={(event) => setDraft(event.target.value)}
              onKeyDown={(event) => {
                if ((event.metaKey || event.ctrlKey) && event.key === "Enter") {
                  event.preventDefault();
                  void submitAdd();
                } else if (event.key === "Escape") {
                  setComposerOpen(false);
                }
              }}
            />
            <div className="flex items-center justify-end gap-1.5">
              <Button
                type="button"
                size="xs"
                variant="ghost"
                disabled={adding}
                onClick={() => {
                  setComposerOpen(false);
                  setDraft("");
                }}
              >
                取消
              </Button>
              <Button type="submit" size="xs" disabled={adding || busy || !draft.trim()}>
                {adding ? <Spinner /> : <Plus />}
                添加
              </Button>
            </div>
          </form>
        ) : (
          <Button
            type="button"
            size="xs"
            variant="ghost"
            disabled={busy}
            className="text-muted-foreground hover:text-foreground"
            onClick={() => setComposerOpen(true)}
          >
            <Plus />
            添加条目
          </Button>
        )}
      </div>

      <AlertDialog open={Boolean(confirmDelete)} onOpenChange={(next) => !next && setConfirmDelete(undefined)}>
        <AlertDialogContent size="sm">
          <AlertDialogHeader>
            <AlertDialogTitle>删除这条记忆？</AlertDialogTitle>
            <AlertDialogDescription className="line-clamp-3 break-words">{confirmDelete}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={deleting}>取消</AlertDialogCancel>
            <AlertDialogAction variant="destructive" disabled={deleting} onClick={() => void confirmRemove()}>
              {deleting ? <Spinner /> : null}
              删除
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}

export function MemorySettingsTab({
  projectId,
  sessionId,
}: {
  projectId?: string | undefined;
  sessionId?: string | undefined;
}) {
  const connected = useLila((state) => state.connection === "connected");
  const models = useLila((state) => state.models);
  const dirty = useLila((state) => state.settingsDirty.memory);
  const [data, setData] = React.useState<MemorySnapshot>();
  const [loading, setLoading] = React.useState(false);
  const [error, setError] = React.useState<string>();
  const [savingConfig, setSavingConfig] = React.useState(false);
  const [reloading, setReloading] = React.useState(false);
  const [activeStore, setActiveStore] = React.useState<MemoryTarget>("memory");
  const [advancedOpen, setAdvancedOpen] = React.useState(false);

  const [memoryLimitInput, setMemoryLimitInput] = React.useState("2200");
  const [userLimitInput, setUserLimitInput] = React.useState("1375");
  const [minHoursInput, setMinHoursInput] = React.useState("24");
  const [minTurnsInput, setMinTurnsInput] = React.useState("5");

  const syncInputs = React.useCallback((snapshot: MemorySnapshot) => {
    setMemoryLimitInput(String(snapshot.memory.charLimit));
    setUserLimitInput(String(snapshot.user.charLimit));
    setMinHoursInput(String(snapshot.dreaming.minHoursSinceLastRun));
    setMinTurnsInput(String(snapshot.dreaming.minTurnsSinceLastRun));
  }, []);

  const load = React.useCallback(async () => {
    if (!connected) return;
    setLoading(true);
    setError(undefined);
    try {
      const snapshot = await actions.getMemory(projectId, sessionId);
      setData(snapshot);
      syncInputs(snapshot);
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setLoading(false);
    }
  }, [connected, projectId, sessionId, syncInputs]);

  React.useEffect(() => {
    void load();
  }, [load]);

  const { enabledModels, groupedModels } = useGroupedModels(models, data?.dreaming.modelId);
  const modelItems = React.useMemo(
    () => [
      { value: AUTO_MODEL_VALUE, label: "跟随当前会话" },
      ...enabledModels.map((model) => ({ value: model.id, label: `${model.providerName} · ${model.label}` })),
    ],
    [enabledModels],
  );

  const patchConfig = async (
    patch: Parameters<typeof actions.updateMemoryConfig>[0],
    successText?: string,
    markDirty = false,
  ) => {
    setSavingConfig(true);
    try {
      const next = await actions.updateMemoryConfig(patch);
      if (next) {
        setData(next);
        syncInputs(next);
        if (markDirty && sessionId) actions.setSettingsDirty("memory", true);
        if (successText) toast.success(successText);
      }
    } finally {
      setSavingConfig(false);
    }
  };

  const handleReload = async () => {
    setReloading(true);
    try {
      if (await actions.reloadWorkspace(projectId, sessionId)) actions.setSettingsDirty("memory", false);
      await load();
    } finally {
      setReloading(false);
    }
  };

  const commitLimits = async () => {
    if (!data) return;
    const nextMemoryLimit = Math.max(500, Math.min(50_000, Math.floor(Number(memoryLimitInput) || data.memory.charLimit)));
    const nextUserLimit = Math.max(500, Math.min(50_000, Math.floor(Number(userLimitInput) || data.user.charLimit)));
    const nextMinHours = Math.max(1, Math.min(720, Math.floor(Number(minHoursInput) || data.dreaming.minHoursSinceLastRun)));
    const nextMinTurns = Math.max(1, Math.min(500, Math.floor(Number(minTurnsInput) || data.dreaming.minTurnsSinceLastRun)));
    if (
      nextMemoryLimit === data.memory.charLimit &&
      nextUserLimit === data.user.charLimit &&
      nextMinHours === data.dreaming.minHoursSinceLastRun &&
      nextMinTurns === data.dreaming.minTurnsSinceLastRun
    ) {
      syncInputs(data);
      return;
    }
    await patchConfig(
      {
        memoryCharLimit: nextMemoryLimit,
        userCharLimit: nextUserLimit,
        dreamingMinHours: nextMinHours,
        dreamingMinTurns: nextMinTurns,
      },
      "已保存参数",
      true,
    );
  };

  const handleAdd = async (target: MemoryTarget, content: string): Promise<boolean> => {
    const next = await actions.addMemory(target, content);
    if (!next) return false;
    setData(next);
    syncInputs(next);
    return true;
  };

  const handleReplace = async (target: MemoryTarget, oldText: string, newContent: string): Promise<boolean> => {
    const next = await actions.replaceMemory(target, oldText, newContent);
    if (!next) return false;
    setData(next);
    syncInputs(next);
    return true;
  };

  const handleRemove = async (target: MemoryTarget, oldText: string): Promise<void> => {
    const next = await actions.removeMemory(target, oldText);
    if (!next) return;
    setData(next);
    syncInputs(next);
  };

  if (loading && !data) {
    return (
      <div className="flex items-center justify-center gap-2 py-12 text-xs text-muted-foreground">
        <Spinner />
        正在加载…
      </div>
    );
  }

  const currentStore = data ? (activeStore === "user" ? data.user : data.memory) : undefined;

  return (
    <div className="flex flex-col gap-6">
      {error ? (
        <Alert variant="destructive">
          <TriangleAlert />
          <AlertTitle>读取失败</AlertTitle>
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      ) : null}

      {data?.externalExtension ? (
        <Alert>
          <Info />
          <AlertTitle>已使用外部记忆扩展</AlertTitle>
          <AlertDescription className="font-mono text-xs">{data.externalExtension}</AlertDescription>
        </Alert>
      ) : null}

      {data && currentStore ? (
        <>
          <SettingsSection>
            <SettingsRow title="内置长期记忆">
              <div className="flex items-center gap-2">
                <IconButton size="icon-xs" label="在访达中打开目录" onClick={() => void actions.revealPath(data.dir)}>
                  <FolderOpen />
                </IconButton>
                {dirty ? (
                  <Button type="button" size="xs" variant="secondary" disabled={reloading} onClick={() => void handleReload()}>
                    {reloading ? <Spinner /> : <RotateCw />}
                    应用更改
                  </Button>
                ) : (
                  <IconButton size="icon-xs" label="刷新" disabled={reloading} onClick={() => void handleReload()}>
                    {reloading ? <Spinner /> : <RotateCw />}
                  </IconButton>
                )}
                <Switch
                  checked={data.enabled}
                  disabled={savingConfig || Boolean(data.externalExtension)}
                  aria-label="内置长期记忆"
                  onCheckedChange={(checked) => void patchConfig({ enabled: checked }, undefined, true)}
                />
              </div>
            </SettingsRow>
          </SettingsSection>

          <section className="flex flex-col gap-3">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <Tabs value={activeStore} onValueChange={(next) => setActiveStore(next as MemoryTarget)}>
                <TabsList>
                  <TabsTrigger value="memory">Agent 笔记 · {data.memory.entries.length}</TabsTrigger>
                  <TabsTrigger value="user">用户画像 · {data.user.entries.length}</TabsTrigger>
                </TabsList>
              </Tabs>
              <div className="flex items-center gap-2">
                <Progress value={currentStore.percent} className="w-16 gap-0" />
                <span className="font-mono text-[11px] text-muted-foreground tabular-nums">
                  {currentStore.charCount.toLocaleString()} / {currentStore.charLimit.toLocaleString()}
                </span>
                <IconButton
                  size="icon-xs"
                  label={`打开 ${currentStore.target === "user" ? "USER.md" : "MEMORY.md"}`}
                  onClick={() =>
                    void window.lila
                      .openPath(currentStore.filePath)
                      .catch((err: unknown) => toast.error(err instanceof Error ? err.message : String(err)))
                  }
                >
                  <ExternalLink />
                </IconButton>
              </div>
            </div>

            <div className="border-t border-border/60">
              <MemoryNotesList
                store={currentStore}
                busy={savingConfig}
                onAdd={(content) => handleAdd(activeStore, content)}
                onReplace={(oldText, newContent) => handleReplace(activeStore, oldText, newContent)}
                onRemove={(oldText) => handleRemove(activeStore, oldText)}
              />
            </div>
          </section>

          <SettingsSection title="后台整理">
            <SettingsRow title="自动提炼">
              <Switch
                checked={data.dreaming.enabled}
                disabled={savingConfig || !data.enabled}
                aria-label="自动提炼"
                onCheckedChange={(checked) => void patchConfig({ dreamingEnabled: checked }, undefined, true)}
              />
            </SettingsRow>

            <SettingsRow title="整理模型">
              <Select
                value={data.dreaming.modelId ?? AUTO_MODEL_VALUE}
                disabled={savingConfig || !data.enabled || !data.dreaming.enabled}
                items={modelItems}
                onValueChange={(value) => {
                  if (!value) return;
                  void patchConfig({ dreamingModelId: value === AUTO_MODEL_VALUE ? null : value }, undefined, true);
                }}
              >
                <SelectTrigger size="sm" className="w-56 text-xs">
                  <SelectValue placeholder="跟随当前会话" />
                </SelectTrigger>
                <SelectContent align="end">
                  <SelectItem value={AUTO_MODEL_VALUE}>跟随当前会话</SelectItem>
                  {groupedModels.map(([providerId, group]) => (
                    <SelectGroup key={providerId}>
                      <SelectLabel>{group.providerName}</SelectLabel>
                      {group.items.map((model) => (
                        <SelectItem key={model.id} value={model.id}>
                          {model.label}
                        </SelectItem>
                      ))}
                    </SelectGroup>
                  ))}
                </SelectContent>
              </Select>
            </SettingsRow>

            <Collapsible open={advancedOpen} onOpenChange={setAdvancedOpen}>
              <CollapsibleTrigger className="flex w-full cursor-pointer items-center justify-between gap-2 px-1 py-2.5 text-xs text-muted-foreground transition-colors hover:text-foreground outline-none focus-visible:text-foreground">
                <span>高级参数</span>
                <span className="flex items-center gap-1 font-mono text-[11px] tabular-nums">
                  {data.dreaming.minHoursSinceLastRun}h · {data.dreaming.minTurnsSinceLastRun} 轮
                  <ChevronRight
                    aria-hidden
                    className={cn(
                      "size-3.5 transition-transform duration-150 ease-(--ease-ui-out) motion-reduce:transition-none",
                      advancedOpen && "rotate-90",
                    )}
                  />
                </span>
              </CollapsibleTrigger>
              <CollapsibleContent className="h-(--collapsible-panel-height) overflow-hidden px-1 transition-[height,opacity] duration-200 ease-(--ease-ui-out) data-ending-style:duration-150 data-ending-style:h-0 data-ending-style:opacity-0 data-starting-style:h-0 data-starting-style:opacity-0 motion-reduce:transition-none">
                <div className="grid grid-cols-2 gap-3 pt-1 pb-3 sm:grid-cols-4">
                  <label className="flex flex-col gap-1">
                    <span className="text-[11px] text-muted-foreground">间隔（小时）</span>
                    <Input
                      type="number"
                      min={1}
                      max={720}
                      value={minHoursInput}
                      disabled={savingConfig || !data.enabled}
                      className="h-8 text-xs tabular-nums"
                      onChange={(event) => setMinHoursInput(event.target.value)}
                      onBlur={() => void commitLimits()}
                    />
                  </label>
                  <label className="flex flex-col gap-1">
                    <span className="text-[11px] text-muted-foreground">最少轮次</span>
                    <Input
                      type="number"
                      min={1}
                      max={500}
                      value={minTurnsInput}
                      disabled={savingConfig || !data.enabled}
                      className="h-8 text-xs tabular-nums"
                      onChange={(event) => setMinTurnsInput(event.target.value)}
                      onBlur={() => void commitLimits()}
                    />
                  </label>
                  <label className="flex flex-col gap-1">
                    <span className="text-[11px] text-muted-foreground">笔记上限</span>
                    <Input
                      type="number"
                      min={500}
                      max={50000}
                      step={200}
                      value={memoryLimitInput}
                      disabled={savingConfig || !data.enabled}
                      className="h-8 text-xs tabular-nums"
                      onChange={(event) => setMemoryLimitInput(event.target.value)}
                      onBlur={() => void commitLimits()}
                    />
                  </label>
                  <label className="flex flex-col gap-1">
                    <span className="text-[11px] text-muted-foreground">画像上限</span>
                    <Input
                      type="number"
                      min={500}
                      max={50000}
                      step={200}
                      value={userLimitInput}
                      disabled={savingConfig || !data.enabled}
                      className="h-8 text-xs tabular-nums"
                      onChange={(event) => setUserLimitInput(event.target.value)}
                      onBlur={() => void commitLimits()}
                    />
                  </label>
                </div>
              </CollapsibleContent>
            </Collapsible>
          </SettingsSection>
        </>
      ) : null}
    </div>
  );
}

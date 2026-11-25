import * as React from "react";
import { ExternalLink, FileJson, Globe, Pencil, Plus, RotateCw, Terminal, Trash2, TriangleAlert } from "lucide-react";
import { toast } from "sonner";
import type { McpConfigSnapshot, McpExposure, McpScope, McpServerItem } from "@lila/protocol";
import { IconButton } from "@/components/icon-button";
import { McpImportForm, McpServerForm } from "@/components/settings/mcp-form";
import {
  emptyMcpForm,
  EXPOSURE_OPTIONS,
  mcpFormFromItem,
  type McpFormErrors,
  type McpFormState,
  parseMcpForm,
  parseMcpJsonImport,
} from "@/components/settings/mcp-parse";
import { SettingsSection } from "@/components/settings/shared";
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
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import { cn } from "cn";
import { actions, useLila } from "@/lib/store";

export function McpSettingsTab({
  projectId,
  projectName,
  sessionId,
}: {
  projectId?: string | undefined;
  projectName?: string | undefined;
  sessionId?: string | undefined;
}) {
  const connected = useLila((state) => state.connection === "connected");
  const dirty = useLila((state) => state.settingsDirty.mcp);
  const [config, setConfig] = React.useState<McpConfigSnapshot>();
  const [loading, setLoading] = React.useState(false);
  const [error, setError] = React.useState<string>();
  const [form, setForm] = React.useState<McpFormState>();
  const [initialFormSnapshot, setInitialFormSnapshot] = React.useState("");
  const [formErrors, setFormErrors] = React.useState<McpFormErrors>({});
  const [formError, setFormError] = React.useState<string>();
  const [importMode, setImportMode] = React.useState(false);
  const [importJson, setImportJson] = React.useState("");
  const [importError, setImportError] = React.useState<string>();
  const [importScope, setImportScope] = React.useState<McpScope>("global");
  const [importing, setImporting] = React.useState(false);
  const [confirmRemove, setConfirmRemove] = React.useState<McpServerItem>();
  const [saving, setSaving] = React.useState(false);
  const [reloading, setReloading] = React.useState(false);
  const [busyKey, setBusyKey] = React.useState<string>();

  const load = React.useCallback(async () => {
    if (!connected) return;
    setLoading(true);
    setError(undefined);
    try {
      setConfig(await actions.listMcp(projectId));
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setLoading(false);
    }
  }, [connected, projectId]);

  React.useEffect(() => {
    void load();
  }, [load]);

  React.useEffect(() => {
    if (!projectId && importScope === "project") setImportScope("global");
  }, [projectId, importScope]);

  const hasDraft = Boolean(form && JSON.stringify(form) !== initialFormSnapshot) || Boolean(importMode && importJson);
  const draftStatus = saving || importing ? "saving" : hasDraft ? "dirty" : undefined;
  React.useLayoutEffect(() => {
    actions.setSettingsDraftStatus(draftStatus);
    return () => actions.setSettingsDraftStatus(undefined);
  }, [draftStatus]);

  const openForm = (next: McpFormState) => {
    setForm(next);
    setInitialFormSnapshot(JSON.stringify(next));
    setFormError(undefined);
    setFormErrors({});
  };

  const closeImport = () => {
    setImportMode(false);
    setImportJson("");
    setImportError(undefined);
    setImportScope("global");
  };

  const openFile = (path: string) => {
    void window.lila
      .openPath(path)
      .catch((failure: unknown) => toast.error(failure instanceof Error ? failure.message : String(failure)));
  };

  const handleSubmit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!form || saving) return;
    setFormError(undefined);
    const parsed = parseMcpForm(form);
    if ("errors" in parsed) {
      setFormErrors(parsed.errors);
      return;
    }
    setFormErrors({});
    setSaving(true);
    const { name, config: input } = parsed.entry;
    try {
      setConfig(await actions.upsertMcp(form.scope, name, input, projectId));
      setForm(undefined);
      actions.setSettingsDirty("mcp", true);
      toast.success(form.editingKey ? `已更新 ${name}` : `已添加 ${name}`);
    } catch (failure) {
      setFormError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setSaving(false);
    }
  };

  const handleImportJson = async (event: React.FormEvent) => {
    event.preventDefault();
    if (importing || !importJson.trim()) return;
    setImportError(undefined);
    setImporting(true);
    try {
      const entries = parseMcpJsonImport(importJson);
      setConfig(await actions.importMcp(importScope, entries, projectId));
      actions.setSettingsDirty("mcp", true);
      closeImport();
      toast.success(`已导入 ${entries.length} 个服务器`);
    } catch (failure) {
      setImportError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setImporting(false);
    }
  };

  const handleToggle = async (item: McpServerItem, patch: { enabled?: boolean; exposure?: McpExposure }) => {
    const key = `${item.scope}:${item.name}`;
    setBusyKey(key);
    try {
      const next = await actions.toggleMcp(item.scope, item.name, patch, projectId);
      if (next) {
        setConfig(next);
        actions.setSettingsDirty("mcp", true);
      }
    } finally {
      setBusyKey(undefined);
    }
  };

  const handleRemove = async (item: McpServerItem) => {
    const key = `${item.scope}:${item.name}`;
    setBusyKey(key);
    try {
      const next = await actions.removeMcp(item.scope, item.name, projectId);
      if (next) {
        setConfig(next);
        actions.setSettingsDirty("mcp", true);
        if (form?.editingKey === key) setForm(undefined);
        toast.success(`已移除 ${item.name}`);
      }
    } finally {
      setBusyKey(undefined);
    }
  };

  const handleReload = async () => {
    setReloading(true);
    try {
      if (await actions.reloadWorkspace(projectId, sessionId)) actions.setSettingsDirty("mcp", false);
      await load();
    } finally {
      setReloading(false);
    }
  };

  if (importMode) {
    return (
      <McpImportForm
        scope={importScope}
        json={importJson}
        projectId={projectId}
        projectName={projectName}
        importing={importing}
        error={importError}
        onScopeChange={setImportScope}
        onJsonChange={(value) => {
          setImportJson(value);
          setImportError(undefined);
        }}
        onCancel={() => actions.requestSettingsNavigation(closeImport)}
        onSubmit={(event) => void handleImportJson(event)}
      />
    );
  }

  if (form) {
    return (
      <McpServerForm
        form={form}
        projectId={projectId}
        projectName={projectName}
        saving={saving}
        errors={formErrors}
        error={formError}
        onChange={(next) => {
          setForm(next);
          setFormError(undefined);
          setFormErrors((errors) => {
            const active = { ...errors };
            for (const field of Object.keys(active) as Array<keyof McpFormErrors>) {
              if (next[field] !== form[field]) delete active[field];
            }
            return active;
          });
        }}
        onCancel={() => actions.requestSettingsNavigation(() => setForm(undefined))}
        onSubmit={(event) => void handleSubmit(event)}
      />
    );
  }

  const servers = config?.servers ?? [];
  const globalServers = servers.filter((item) => item.scope === "global");
  const projectServers = servers.filter((item) => item.scope === "project");

  const renderServer = (item: McpServerItem) => {
    const key = `${item.scope}:${item.name}`;
    const busy = busyKey === key;
    const endpoint = item.transport === "http" ? item.url : [item.command, ...(item.args ?? [])].filter(Boolean).join(" ");
    const TransportIcon = item.transport === "http" ? Globe : Terminal;

    return (
      <div key={key} className="flex flex-wrap items-center justify-between gap-3 px-1 py-2.5">
        <div className={cn("flex min-w-0 flex-1 items-start gap-2.5", !item.enabled && "opacity-55")}>
          <TransportIcon className="mt-1 size-3.5 shrink-0 text-muted-foreground" />
          <div className="flex min-w-0 flex-1 flex-col gap-0.5">
            <span className="truncate font-mono text-sm text-foreground">{item.name}</span>
            {endpoint ? (
              <span className="truncate font-mono text-[11px] text-muted-foreground" title={endpoint}>
                {endpoint}
              </span>
            ) : null}
          </div>
        </div>

        <div className="flex shrink-0 items-center gap-1.5">
          <Select
            value={item.exposure}
            disabled={busy}
            onValueChange={(value) => {
              if (value && value !== item.exposure) void handleToggle(item, { exposure: value as McpExposure });
            }}
            items={EXPOSURE_OPTIONS}
          >
            <SelectTrigger size="sm" className="h-7 w-24 text-xs">
              <SelectValue />
            </SelectTrigger>
            <SelectContent align="end">
              {EXPOSURE_OPTIONS.map((opt) => (
                <SelectItem key={opt.value} value={opt.value}>
                  {opt.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Switch
            checked={item.enabled}
            disabled={busy}
            aria-label={`${item.enabled ? "禁用" : "启用"} ${item.name}`}
            onCheckedChange={(checked) => void handleToggle(item, { enabled: checked })}
          />
          <IconButton size="icon-xs" label="编辑" disabled={busy} onClick={() => openForm(mcpFormFromItem(item))}>
            <Pencil />
          </IconButton>
          <IconButton size="icon-xs" label="删除" disabled={busy} className="hover:text-destructive" onClick={() => setConfirmRemove(item)}>
            <Trash2 />
          </IconButton>
        </div>
      </div>
    );
  };

  return (
    <div className="flex flex-col gap-5">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="flex flex-wrap items-center gap-2">
          <Button type="button" size="sm" onClick={() => openForm(emptyMcpForm("global"))}>
            <Plus />
            添加
          </Button>
          <Button type="button" size="sm" variant="outline" onClick={() => setImportMode(true)}>
            <FileJson />
            导入 JSON
          </Button>
          <Button type="button" size="sm" variant={dirty ? "secondary" : "outline"} disabled={reloading} onClick={() => void handleReload()}>
            {reloading ? <Spinner /> : <RotateCw />}
            {dirty ? "应用更改" : "刷新"}
          </Button>
        </div>
        {config ? (
          <div className="flex items-center gap-1">
            <Button type="button" size="xs" variant="ghost" className="text-muted-foreground" onClick={() => openFile(config.globalPath)}>
              <ExternalLink />
              全局 mcp.json
            </Button>
            {config.projectPath ? (
              <Button type="button" size="xs" variant="ghost" className="text-muted-foreground" onClick={() => openFile(config.projectPath!)}>
                <ExternalLink />
                项目 mcp.json
              </Button>
            ) : null}
          </div>
        ) : null}
      </div>

      {config?.errors.length ? (
        <Alert variant="destructive">
          <TriangleAlert />
          <AlertTitle>配置有 {config.errors.length} 个问题</AlertTitle>
          <AlertDescription>
            <ul className="list-disc ps-4 text-xs">
              {config.errors.map((line, index) => (
                <li key={index} className="break-words">
                  {line}
                </li>
              ))}
            </ul>
          </AlertDescription>
        </Alert>
      ) : null}

      {loading && !config ? (
        <div className="flex items-center justify-center gap-2 py-12 text-xs text-muted-foreground">
          <Spinner />
          正在加载…
        </div>
      ) : error ? (
        <Empty className="py-10">
          <EmptyHeader>
            <EmptyTitle>读取失败</EmptyTitle>
            <EmptyDescription>{error}</EmptyDescription>
          </EmptyHeader>
          <Button type="button" size="sm" variant="outline" onClick={() => void load()}>
            <RotateCw />
            重试
          </Button>
        </Empty>
      ) : !servers.length ? (
        <Empty className="py-12">
          <EmptyHeader>
            <EmptyTitle>暂无 MCP 服务器</EmptyTitle>
          </EmptyHeader>
          <Button type="button" size="sm" variant="outline" onClick={() => openForm(emptyMcpForm("global"))}>
            <Plus />
            添加服务器
          </Button>
        </Empty>
      ) : (
        <div className="flex flex-col gap-6">
          {projectServers.length ? (
            <SettingsSection title={projectName ? `项目 · ${projectName}` : "当前项目"}>
              {projectServers.map(renderServer)}
            </SettingsSection>
          ) : null}
          {globalServers.length ? <SettingsSection title="全局">{globalServers.map(renderServer)}</SettingsSection> : null}
        </div>
      )}

      <AlertDialog open={Boolean(confirmRemove)} onOpenChange={(next) => !next && setConfirmRemove(undefined)}>
        <AlertDialogContent size="sm">
          <AlertDialogHeader>
            <AlertDialogTitle>删除「{confirmRemove?.name}」？</AlertDialogTitle>
            <AlertDialogDescription>将从对应 mcp.json 中移除。</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>取消</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={() => {
                const target = confirmRemove;
                setConfirmRemove(undefined);
                if (target) void handleRemove(target);
              }}
            >
              删除
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

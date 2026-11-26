import * as React from "react";
import { ArrowUpCircle, ExternalLink, FolderOpen, Package, Plus, RotateCw, Trash2, TriangleAlert } from "lucide-react";
import { toast } from "sonner";
import type { CapabilitiesSnapshot, PackageScope } from "@lila/protocol";
import { IconButton } from "@/components/icon-button";
import { SearchInput } from "@/components/search-input";
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
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { actions, useLila } from "@/lib/store";

function ResourceRow({
  title,
  description,
  meta,
  path,
  revealOnly,
}: {
  title: React.ReactNode;
  description?: string | undefined;
  meta?: string | undefined;
  path?: string | undefined;
  revealOnly?: boolean;
}) {
  return (
    <div className="group/row flex items-start justify-between gap-2 px-1 py-2.5 hover:bg-muted/35">
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <div className="flex min-w-0 items-baseline gap-2">
          <span className="min-w-0 truncate font-mono text-xs font-medium text-foreground">{title}</span>
          {meta ? <span className="shrink-0 text-[11px] text-muted-foreground">{meta}</span> : null}
        </div>
        {description ? <p className="line-clamp-2 text-xs text-muted-foreground">{description}</p> : null}
      </div>
      {path ? (
        <IconButton
          size="icon-xs"
          label={revealOnly ? "在访达中显示" : "打开"}
          className="shrink-0 opacity-60 transition-opacity duration-150 group-hover/row:opacity-100 focus-visible:opacity-100 pointer-coarse:opacity-100"
          onClick={() => {
            if (revealOnly) void actions.revealPath(path);
            else void window.lila.openPath(path).catch((error: unknown) => toast.error(error instanceof Error ? error.message : String(error)));
          }}
        >
          {revealOnly ? <FolderOpen /> : <ExternalLink />}
        </IconButton>
      ) : null}
    </div>
  );
}

export function CapabilitiesSettingsTab({
  projectId,
  projectName,
  sessionId,
}: {
  projectId?: string | undefined;
  projectName?: string | undefined;
  sessionId?: string | undefined;
}) {
  const connected = useLila((state) => state.connection === "connected");
  const dirty = useLila((state) => state.settingsDirty.capabilities);
  const [data, setData] = React.useState<CapabilitiesSnapshot>();
  const [loading, setLoading] = React.useState(false);
  const [error, setError] = React.useState<string>();
  const [reloading, setReloading] = React.useState(false);

  const [source, setSource] = React.useState("");
  const [scope, setScope] = React.useState<PackageScope>("user");
  const [installing, setInstalling] = React.useState(false);
  const [updatingSource, setUpdatingSource] = React.useState<string | "__all__">();
  const [removingKey, setRemovingKey] = React.useState<string>();
  const [confirmRemove, setConfirmRemove] = React.useState<{ source: string; scope: PackageScope }>();
  const [query, setQuery] = React.useState("");

  React.useEffect(() => {
    if (!projectId && scope === "project") setScope("user");
  }, [projectId, scope]);

  const load = React.useCallback(async () => {
    if (!connected) return;
    setLoading(true);
    setError(undefined);
    try {
      setData(await actions.getCapabilities(projectId, sessionId));
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setLoading(false);
    }
  }, [connected, projectId, sessionId]);

  React.useEffect(() => {
    void load();
  }, [load]);

  const handleInstall = async (event: React.FormEvent) => {
    event.preventDefault();
    const trimmed = source.trim();
    if (!trimmed || installing) return;
    setInstalling(true);
    try {
      const next = await actions.installPackage(trimmed, scope, projectId);
      if (next) {
        setData(next);
        setSource("");
        actions.setSettingsDirty("capabilities", true);
        toast.success(`已安装 ${trimmed}`);
      }
    } finally {
      setInstalling(false);
    }
  };

  const handleRemove = async (pkgSource: string, pkgScope: PackageScope) => {
    const key = `${pkgScope}:${pkgSource}`;
    setRemovingKey(key);
    try {
      const next = await actions.removePackage(pkgSource, pkgScope, projectId);
      if (next) {
        setData(next);
        actions.setSettingsDirty("capabilities", true);
        toast.success(`已卸载 ${pkgSource}`);
      }
    } finally {
      setRemovingKey(undefined);
    }
  };

  const handleUpdate = async (pkgSource?: string) => {
    setUpdatingSource(pkgSource ?? "__all__");
    try {
      const next = await actions.updatePackages(pkgSource, projectId);
      if (next) {
        setData(next);
        actions.setSettingsDirty("capabilities", true);
        toast.success(pkgSource ? `已更新 ${pkgSource}` : "已更新全部扩展包");
      }
    } finally {
      setUpdatingSource(undefined);
    }
  };

  const handleReload = async () => {
    setReloading(true);
    try {
      if (await actions.reloadWorkspace(projectId, sessionId)) actions.setSettingsDirty("capabilities", false);
      await load();
    } finally {
      setReloading(false);
    }
  };

  const needle = query.trim().toLowerCase();
  const filteredSkills = React.useMemo(
    () => (data?.skills ?? []).filter((s) => !needle || s.name.toLowerCase().includes(needle) || s.description.toLowerCase().includes(needle)),
    [data?.skills, needle],
  );
  const filteredPrompts = React.useMemo(
    () => (data?.prompts ?? []).filter((p) => !needle || p.name.toLowerCase().includes(needle) || p.description.toLowerCase().includes(needle)),
    [data?.prompts, needle],
  );
  const filteredExtensions = React.useMemo(
    () =>
      (data?.extensions ?? []).filter(
        (ext) =>
          !needle ||
          ext.path.toLowerCase().includes(needle) ||
          ext.tools.some((t) => t.toLowerCase().includes(needle)) ||
          ext.commands.some((c) => c.toLowerCase().includes(needle)),
      ),
    [data?.extensions, needle],
  );
  const filteredContext = React.useMemo(
    () => (data?.contextFiles ?? []).filter((f) => !needle || f.toLowerCase().includes(needle)),
    [data?.contextFiles, needle],
  );

  return (
    <div className="flex flex-col gap-6">
      {data?.diagnostics.length ? (
        <Alert variant="destructive">
          <TriangleAlert />
          <AlertTitle>加载有 {data.diagnostics.length} 个问题</AlertTitle>
          <AlertDescription>
            <ul className="list-disc ps-4 text-xs">
              {data.diagnostics.map((line, index) => (
                <li key={index} className="break-words whitespace-pre-wrap">
                  {line}
                </li>
              ))}
            </ul>
          </AlertDescription>
        </Alert>
      ) : null}

      <section className="flex flex-col gap-2.5">
        <div className="flex items-center justify-between gap-2 px-1">
          <h3 className="text-xs font-medium text-muted-foreground">扩展包</h3>
          <div className="flex items-center gap-1">
            {data?.packages.length ? (
              <Button type="button" size="xs" variant="ghost" disabled={Boolean(updatingSource)} onClick={() => void handleUpdate()}>
                {updatingSource === "__all__" ? <Spinner /> : <ArrowUpCircle />}
                全部更新
              </Button>
            ) : null}
            <Button type="button" size="xs" variant={dirty ? "secondary" : "ghost"} disabled={reloading} onClick={() => void handleReload()}>
              {reloading ? <Spinner /> : <RotateCw />}
              {dirty ? "应用更改" : "刷新"}
            </Button>
          </div>
        </div>

        <form onSubmit={(event) => void handleInstall(event)} className="flex flex-col gap-2 sm:flex-row sm:items-center">
          <Input
            value={source}
            disabled={installing}
            placeholder="npm:@scope/pkg 或 git 仓库地址"
            aria-label="安装扩展包"
            className="h-8 min-w-0 flex-1 font-mono text-xs"
            onChange={(event) => setSource(event.target.value)}
          />
          <div className="flex items-center gap-1.5">
            <Select
              value={scope}
              disabled={!projectId || installing}
              onValueChange={(value) => {
                if (value) setScope(value as PackageScope);
              }}
              items={[
                { value: "user", label: "全局" },
                ...(projectId ? [{ value: "project", label: projectName || "当前项目" }] : []),
              ]}
            >
              <SelectTrigger size="sm" className="h-8 w-24 text-xs">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="user">全局</SelectItem>
                {projectId ? <SelectItem value="project">{projectName || "当前项目"}</SelectItem> : null}
              </SelectContent>
            </Select>
            <Button type="submit" size="sm" disabled={installing || !source.trim()}>
              {installing ? <Spinner /> : <Plus />}
              安装
            </Button>
          </div>
        </form>

        {data?.packages.length ? (
          <SettingsSection>
            {data.packages.map((pkg) => {
              const key = `${pkg.scope}:${pkg.source}`;
              const removing = removingKey === key;
              return (
                <div key={key} className="flex items-center justify-between gap-2 px-1 py-2">
                  <div className="flex min-w-0 flex-1 items-baseline gap-2">
                    <Package className="size-3.5 shrink-0 self-center text-muted-foreground" />
                    <span className="truncate font-mono text-xs text-foreground">{pkg.source}</span>
                    {pkg.scope === "project" ? <span className="shrink-0 text-[11px] text-muted-foreground">项目</span> : null}
                  </div>
                  <div className="flex shrink-0 items-center gap-0.5">
                    <IconButton
                      size="icon-xs"
                      label="更新"
                      disabled={Boolean(updatingSource) || removing}
                      onClick={() => void handleUpdate(pkg.source)}
                    >
                      {updatingSource === pkg.source ? <Spinner /> : <ArrowUpCircle />}
                    </IconButton>
                    {pkg.installedPath ? (
                      <IconButton size="icon-xs" label="在访达中显示" onClick={() => void actions.revealPath(pkg.installedPath!)}>
                        <FolderOpen />
                      </IconButton>
                    ) : null}
                    <IconButton
                      size="icon-xs"
                      label="卸载"
                      disabled={removing}
                      className="hover:text-destructive"
                      onClick={() => setConfirmRemove({ source: pkg.source, scope: pkg.scope })}
                    >
                      {removing ? <Spinner /> : <Trash2 />}
                    </IconButton>
                  </div>
                </div>
              );
            })}
          </SettingsSection>
        ) : null}
      </section>

      {loading && !data ? (
        <div className="flex items-center justify-center gap-2 py-8 text-xs text-muted-foreground">
          <Spinner />
          正在加载…
        </div>
      ) : error ? (
        <Empty className="py-8">
          <EmptyHeader>
            <EmptyTitle>读取失败</EmptyTitle>
            <EmptyDescription>{error}</EmptyDescription>
          </EmptyHeader>
          <Button type="button" size="sm" variant="outline" onClick={() => void load()}>
            <RotateCw />
            重试
          </Button>
        </Empty>
      ) : data ? (
        <div className="flex flex-col gap-3">
          <SearchInput value={query} onChange={setQuery} placeholder="搜索技能、模板、扩展或上下文" ariaLabel="搜索已加载能力" />

          <Tabs defaultValue="skills" className="gap-2">
            <TabsList>
              <TabsTrigger value="skills">技能 · {filteredSkills.length}</TabsTrigger>
              <TabsTrigger value="prompts">模板 · {filteredPrompts.length}</TabsTrigger>
              <TabsTrigger value="extensions">扩展 · {filteredExtensions.length}</TabsTrigger>
              <TabsTrigger value="context">上下文 · {filteredContext.length}</TabsTrigger>
            </TabsList>

            <TabsContent value="skills" className="divide-y divide-border/50 border-t border-border/60">
              {filteredSkills.length ? (
                filteredSkills.map((skill) => (
                  <ResourceRow
                    key={skill.path}
                    title={skill.name}
                    description={skill.description}
                    path={skill.path}
                    meta={skill.scope}
                  />
                ))
              ) : (
                <p className="px-1 py-6 text-center text-xs text-muted-foreground">暂无技能</p>
              )}
            </TabsContent>

            <TabsContent value="prompts" className="divide-y divide-border/50 border-t border-border/60">
              {filteredPrompts.length ? (
                filteredPrompts.map((prompt) => (
                  <ResourceRow key={prompt.path} title={`/${prompt.name}`} description={prompt.description} path={prompt.path} />
                ))
              ) : (
                <p className="px-1 py-6 text-center text-xs text-muted-foreground">暂无模板</p>
              )}
            </TabsContent>

            <TabsContent value="extensions" className="divide-y divide-border/50 border-t border-border/60">
              {filteredExtensions.length ? (
                filteredExtensions.map((ext) => {
                  const items = [...ext.tools, ...ext.commands.map((c) => `/${c}`)];
                  return (
                    <ResourceRow
                      key={ext.path}
                      title={ext.path}
                      description={items.length ? items.join(" · ") : undefined}
                      path={ext.path}
                      revealOnly
                    />
                  );
                })
              ) : (
                <p className="px-1 py-6 text-center text-xs text-muted-foreground">暂无扩展</p>
              )}
            </TabsContent>

            <TabsContent value="context" className="divide-y divide-border/50 border-t border-border/60">
              {filteredContext.length ? (
                filteredContext.map((filePath) => <ResourceRow key={filePath} title={filePath} path={filePath} />)
              ) : (
                <p className="px-1 py-6 text-center text-xs text-muted-foreground">暂无上下文文件</p>
              )}
            </TabsContent>
          </Tabs>
        </div>
      ) : null}

      <AlertDialog open={Boolean(confirmRemove)} onOpenChange={(next) => !next && setConfirmRemove(undefined)}>
        <AlertDialogContent size="sm">
          <AlertDialogHeader>
            <AlertDialogTitle>卸载「{confirmRemove?.source}」？</AlertDialogTitle>
            <AlertDialogDescription>其提供的技能与扩展将不再加载。</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>取消</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={() => {
                const target = confirmRemove;
                setConfirmRemove(undefined);
                if (target) void handleRemove(target.source, target.scope);
              }}
            >
              卸载
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

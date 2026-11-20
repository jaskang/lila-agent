import * as React from "react";
import { ExternalLink, FolderOpen, RotateCw, TriangleAlert } from "lucide-react";
import type { SessionResources } from "@lila/protocol";
import { parseMcpName } from "@/components/tool-call";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { Spinner } from "@/components/ui/spinner";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { actions } from "@/lib/store";

function Row({ title, description, meta, path }: { title: React.ReactNode; description?: string | undefined; meta?: React.ReactNode; path?: string | undefined }) {
  return (
    <div className="group/row flex items-start gap-2 rounded-lg px-2 py-2 hover:bg-muted/60">
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <div className="flex min-w-0 items-center gap-1.5">
          <span className="min-w-0 truncate font-mono text-xs font-medium">{title}</span>
          {meta}
        </div>
        {description ? <p className="line-clamp-2 text-xs text-muted-foreground">{description}</p> : null}
      </div>
      {path ? (
        <Button size="icon-xs" variant="ghost" className="opacity-0 group-hover/row:opacity-100" title="打开" aria-label="打开" onClick={() => void window.lila.openPath(path)}>
          <ExternalLink />
        </Button>
      ) : null}
    </div>
  );
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="flex flex-col gap-1">
      <h3 className="px-2 text-xs font-medium text-muted-foreground">{title}</h3>
      {children}
    </section>
  );
}

function Nothing({ text }: { text: string }) {
  return <p className="px-2 py-1 text-xs text-muted-foreground">{text}</p>;
}

function McpTab({ resources, reloading, onReload }: { resources: SessionResources; reloading: boolean; onReload: () => void }) {
  const servers = React.useMemo(() => {
    const map = new Map<string, SessionResources["tools"]>();
    for (const tool of resources.tools) {
      const parsed = parseMcpName(tool.name);
      if (parsed) map.set(parsed.server, [...(map.get(parsed.server) ?? []), tool]);
    }
    return [...map].sort(([a], [b]) => a.localeCompare(b));
  }, [resources.tools]);
  return (
    <div className="flex flex-col gap-4">
      <Section title="配置文件">
        {resources.mcpConfigPaths.map((path, index) => (
          <div key={path} className="flex items-center gap-2 rounded-lg px-2 py-1.5 hover:bg-muted/60">
            <Badge variant="outline">{index === 0 ? "用户" : "项目"}</Badge>
            <span className="min-w-0 flex-1 truncate font-mono text-xs" title={path}>
              {path}
            </span>
            <Button size="xs" variant="outline" onClick={() => void window.lila.openPath(path)}>
              打开
            </Button>
          </div>
        ))}
        <div className="px-2 pt-1">
          <Button size="sm" variant="secondary" disabled={reloading} onClick={onReload}>
            {reloading ? <Spinner /> : <RotateCw />}
            重新加载
          </Button>
          <p className="mt-1 text-xs text-muted-foreground">编辑 mcp.json 后重新加载，新的服务器和工具会在下一条消息生效。</p>
        </div>
      </Section>
      {servers.length ? (
        servers.map(([server, tools]) => (
          <Section key={server} title={`${server} · ${tools.length} 个工具`}>
            {tools.map((tool) => (
              <Row
                key={tool.name}
                title={parseMcpName(tool.name)?.tool ?? tool.name}
                description={tool.description}
                meta={tool.active ? null : <Badge variant="outline">未启用</Badge>}
              />
            ))}
          </Section>
        ))
      ) : (
        <Nothing text="没有直接暴露的 MCP 工具。服务器也可能通过 codemode 或 tool_search 间接使用。" />
      )}
    </div>
  );
}

export function ResourcesSheet({ sessionId, open, onOpenChange }: { sessionId: string; open: boolean; onOpenChange: (open: boolean) => void }) {
  const [resources, setResources] = React.useState<SessionResources>();
  const [error, setError] = React.useState<string>();
  const [loading, setLoading] = React.useState(false);
  const [reloading, setReloading] = React.useState(false);

  const load = React.useCallback(async () => {
    setLoading(true);
    setError(undefined);
    try {
      setResources(await actions.resources(sessionId));
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setLoading(false);
    }
  }, [sessionId]);

  React.useEffect(() => {
    if (open) void load();
  }, [open, load]);

  async function reload(): Promise<void> {
    setReloading(true);
    if (await actions.reload(sessionId)) await load();
    setReloading(false);
  }

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent className="gap-0 data-[side=right]:w-[min(32rem,90vw)] data-[side=right]:sm:max-w-none">
        <SheetHeader className="border-b">
          <SheetTitle>能力</SheetTitle>
          <SheetDescription className="truncate font-mono text-xs" title={resources?.cwd}>
            {resources?.cwd ?? "技能、工具、MCP 和扩展"}
          </SheetDescription>
        </SheetHeader>
        {!resources ? (
          <Empty className="flex-1">
            <EmptyHeader>
              <EmptyTitle>{error ? "读取失败" : "正在读取…"}</EmptyTitle>
              {error ? <EmptyDescription>{error}</EmptyDescription> : null}
            </EmptyHeader>
            {error ? (
              <Button variant="outline" onClick={() => void load()}>
                <RotateCw />
                重试
              </Button>
            ) : (
              <Spinner />
            )}
          </Empty>
        ) : (
          <Tabs defaultValue="skills" className="min-h-0 flex-1 gap-0">
            <div className="flex items-center gap-2 px-4 pt-3">
              <TabsList>
                <TabsTrigger value="skills">技能</TabsTrigger>
                <TabsTrigger value="tools">工具</TabsTrigger>
                <TabsTrigger value="mcp">MCP</TabsTrigger>
                <TabsTrigger value="extensions">扩展</TabsTrigger>
                <TabsTrigger value="context">上下文</TabsTrigger>
              </TabsList>
              {loading ? <Spinner className="text-muted-foreground" /> : null}
            </div>
            {resources.diagnostics.length ? (
              <Alert variant="destructive" className="mx-4 mt-3 w-auto">
                <TriangleAlert />
                <AlertTitle>加载时有 {resources.diagnostics.length} 个问题</AlertTitle>
                <AlertDescription>
                  <ul className="list-disc ps-4 text-xs">
                    {resources.diagnostics.map((line, index) => (
                      <li key={index} className="break-words whitespace-pre-wrap">
                        {line}
                      </li>
                    ))}
                  </ul>
                </AlertDescription>
              </Alert>
            ) : null}
            <ScrollArea className="min-h-0 flex-1">
              <div className="p-3">
                <TabsContent value="skills" className="flex flex-col gap-4">
                  <Section title={`技能 · ${resources.skills.length}`}>
                    {resources.skills.length ? (
                      resources.skills.map((skill) => (
                        <Row key={skill.path} title={skill.name} description={skill.description} path={skill.path} meta={<Badge variant="outline">{skill.scope}</Badge>} />
                      ))
                    ) : (
                      <Nothing text="没有技能。" />
                    )}
                  </Section>
                  <Section title={`提示模板 · ${resources.prompts.length}`}>
                    {resources.prompts.length ? (
                      resources.prompts.map((prompt) => <Row key={prompt.path} title={`/${prompt.name}`} description={prompt.description} path={prompt.path} />)
                    ) : (
                      <Nothing text="没有提示模板。" />
                    )}
                  </Section>
                </TabsContent>
                <TabsContent value="tools" className="flex flex-col gap-1">
                  {resources.tools.map((tool) => (
                    <Row
                      key={tool.name}
                      title={tool.name}
                      description={tool.description}
                      meta={
                        <>
                          <Badge variant="secondary">{tool.source}</Badge>
                          {tool.active ? null : <Badge variant="outline">未启用</Badge>}
                        </>
                      }
                    />
                  ))}
                </TabsContent>
                <TabsContent value="mcp">
                  <McpTab resources={resources} reloading={reloading} onReload={() => void reload()} />
                </TabsContent>
                <TabsContent value="extensions" className="flex flex-col gap-1">
                  {resources.extensions.length ? (
                    resources.extensions.map((extension) => (
                      <div key={extension.path} className="flex flex-col gap-1 rounded-lg px-2 py-2 hover:bg-muted/60">
                        <div className="flex items-center gap-2">
                          <span className="min-w-0 flex-1 truncate font-mono text-xs font-medium" title={extension.path}>
                            {extension.path}
                          </span>
                          <Button size="icon-xs" variant="ghost" title="在访达中显示" aria-label="在访达中显示" onClick={() => window.lila.revealPath(extension.path)}>
                            <FolderOpen />
                          </Button>
                        </div>
                        <div className="flex flex-wrap gap-1">
                          {extension.tools.map((tool) => (
                            <Badge key={`t-${tool}`} variant="secondary">
                              {tool}
                            </Badge>
                          ))}
                          {extension.commands.map((command) => (
                            <Badge key={`c-${command}`} variant="outline">
                              /{command}
                            </Badge>
                          ))}
                        </div>
                      </div>
                    ))
                  ) : (
                    <Nothing text="没有扩展。" />
                  )}
                </TabsContent>
                <TabsContent value="context" className="flex flex-col gap-4">
                  <Section title="上下文文件">
                    {resources.contextFiles.length ? (
                      resources.contextFiles.map((path) => <Row key={path} title={path} path={path} />)
                    ) : (
                      <Nothing text="没有 AGENTS.md 等上下文文件。" />
                    )}
                  </Section>
                  <Section title="目录">
                    <Row title={resources.cwd} description="工作目录" path={resources.cwd} />
                    <Row title={resources.agentDir} description="Pi 配置目录（与 pi 命令行共享）" path={resources.agentDir} />
                  </Section>
                </TabsContent>
              </div>
            </ScrollArea>
          </Tabs>
        )}
      </SheetContent>
    </Sheet>
  );
}

import * as React from "react";
import { ArrowLeft, ChevronRight, FileJson, FolderOpen, Globe, Terminal } from "lucide-react";
import type { McpExposure, McpScope } from "@lila/protocol";
import { EXPOSURE_OPTIONS, type McpFormState } from "@/components/settings/mcp-parse";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "cn";

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-1.5">
      <label className="text-xs font-medium text-foreground">{label}</label>
      {children}
    </div>
  );
}

export function McpServerForm({
  form,
  projectId,
  projectName,
  saving,
  onChange,
  onCancel,
  onSubmit,
}: {
  form: McpFormState;
  projectId?: string | undefined;
  projectName?: string | undefined;
  saving: boolean;
  onChange: (next: McpFormState) => void;
  onCancel: () => void;
  onSubmit: (event: React.FormEvent) => void;
}) {
  const isEditing = Boolean(form.editingKey);
  const hasAdvanced = Boolean(
    form.cwd.trim() || form.envText.trim() || form.headersText.trim() || form.description.trim() || form.exposure !== "codemode",
  );
  const [advancedOpen, setAdvancedOpen] = React.useState(hasAdvanced);

  return (
    <form onSubmit={onSubmit} className="flex flex-col gap-5">
      <div className="flex items-center justify-between gap-2">
        <div className="flex min-w-0 items-center gap-2">
          <Button type="button" size="sm" variant="ghost" className="-ms-2 text-muted-foreground hover:text-foreground" onClick={onCancel}>
            <ArrowLeft />
            返回
          </Button>
          <span className="truncate text-sm font-medium text-foreground">{isEditing ? form.name : "添加 MCP"}</span>
        </div>
        {isEditing ? (
          <Switch
            checked={form.enabled}
            disabled={saving}
            aria-label="启用状态"
            onCheckedChange={(checked) => onChange({ ...form, enabled: checked })}
          />
        ) : null}
      </div>

      <div className="flex flex-col gap-4 border-y border-border/60 py-4">
        {!isEditing ? (
          <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
            <Field label="名称">
              <Input
                value={form.name}
                disabled={saving}
                placeholder="filesystem"
                className="h-9 font-mono text-xs"
                onChange={(event) => onChange({ ...form, name: event.target.value })}
              />
            </Field>
            <Field label="作用域">
              <Select
                value={form.scope}
                disabled={!projectId || saving}
                onValueChange={(value) => {
                  if (value) onChange({ ...form, scope: value as McpScope });
                }}
                items={[
                  { value: "global", label: "全局" },
                  ...(projectId ? [{ value: "project", label: projectName || "当前项目" }] : []),
                ]}
              >
                <SelectTrigger className="h-9 w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="global">全局</SelectItem>
                  {projectId ? <SelectItem value="project">{projectName || "当前项目"}</SelectItem> : null}
                </SelectContent>
              </Select>
            </Field>
          </div>
        ) : null}

        <Field label="连接方式">
          <div className="inline-flex w-fit rounded-lg bg-muted/60 p-0.5">
            <Button
              type="button"
              size="xs"
              variant="ghost"
              className={cn(
                "h-7 gap-1.5 px-3 text-xs font-normal text-muted-foreground hover:text-foreground",
                form.transport === "stdio" && "bg-background font-medium text-foreground shadow-2xs",
              )}
              onClick={() => onChange({ ...form, transport: "stdio" })}
            >
              <Terminal className="size-3.5" />
              本地命令
            </Button>
            <Button
              type="button"
              size="xs"
              variant="ghost"
              className={cn(
                "h-7 gap-1.5 px-3 text-xs font-normal text-muted-foreground hover:text-foreground",
                form.transport === "http" && "bg-background font-medium text-foreground shadow-2xs",
              )}
              onClick={() => onChange({ ...form, transport: "http" })}
            >
              <Globe className="size-3.5" />
              HTTP
            </Button>
          </div>
        </Field>

        {form.transport === "stdio" ? (
          <>
            <Field label="命令">
              <Input
                value={form.command}
                disabled={saving}
                placeholder="npx"
                className="h-9 font-mono text-xs"
                onChange={(event) => onChange({ ...form, command: event.target.value })}
              />
            </Field>
            <Field label="参数">
              <Textarea
                value={form.argsText}
                disabled={saving}
                rows={2}
                placeholder={"-y\n@modelcontextprotocol/server-filesystem\n."}
                className="min-h-16 font-mono text-xs"
                onChange={(event) => onChange({ ...form, argsText: event.target.value })}
              />
            </Field>
          </>
        ) : (
          <Field label="URL">
            <Input
              value={form.url}
              disabled={saving}
              placeholder="https://example.com/mcp"
              className="h-9 font-mono text-xs"
              onChange={(event) => onChange({ ...form, url: event.target.value })}
            />
          </Field>
        )}
      </div>

      <Collapsible open={advancedOpen} onOpenChange={setAdvancedOpen} className="flex flex-col gap-3">
        <CollapsibleTrigger
          render={
            <button
              type="button"
              className="flex w-fit items-center gap-1 text-xs font-medium text-muted-foreground transition-colors hover:text-foreground"
            />
          }
        >
          <ChevronRight className={cn("size-3.5 transition-transform", advancedOpen && "rotate-90")} />
          更多选项
        </CollapsibleTrigger>
        <CollapsibleContent className="flex flex-col gap-4 border-t border-border/60 pt-4">
          <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
            <Field label="备注">
              <Input
                value={form.description}
                disabled={saving}
                placeholder="简短备注"
                className="h-9 text-xs"
                onChange={(event) => onChange({ ...form, description: event.target.value })}
              />
            </Field>
            <Field label="暴露方式">
              <Select
                value={form.exposure}
                disabled={saving}
                onValueChange={(value) => {
                  if (value) onChange({ ...form, exposure: value as McpExposure });
                }}
                items={EXPOSURE_OPTIONS}
              >
                <SelectTrigger className="h-9 w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {EXPOSURE_OPTIONS.map((opt) => (
                    <SelectItem key={opt.value} value={opt.value}>
                      {opt.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </Field>
          </div>

          {form.transport === "stdio" ? (
            <>
              <Field label="工作目录">
                <div className="flex items-center gap-2">
                  <Input
                    value={form.cwd}
                    disabled={saving}
                    placeholder="默认项目目录"
                    className="h-9 min-w-0 flex-1 font-mono text-xs"
                    onChange={(event) => onChange({ ...form, cwd: event.target.value })}
                  />
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    disabled={saving}
                    onClick={() =>
                      void window.lila.pickDirectory().then((picked) => {
                        if (picked) onChange({ ...form, cwd: picked });
                      })
                    }
                  >
                    <FolderOpen />
                    选择
                  </Button>
                </div>
              </Field>
              <Field label="环境变量">
                <Textarea
                  value={form.envText}
                  disabled={saving}
                  rows={3}
                  placeholder={"API_TOKEN=${MY_TOKEN}"}
                  className="min-h-20 font-mono text-xs"
                  onChange={(event) => onChange({ ...form, envText: event.target.value })}
                />
              </Field>
            </>
          ) : (
            <Field label="Headers">
              <Textarea
                value={form.headersText}
                disabled={saving}
                rows={3}
                placeholder={"Authorization: Bearer ${DOCS_TOKEN}"}
                className="min-h-20 font-mono text-xs"
                onChange={(event) => onChange({ ...form, headersText: event.target.value })}
              />
            </Field>
          )}
        </CollapsibleContent>
      </Collapsible>

      <div className="flex items-center justify-end gap-2 pt-2">
        <Button type="button" size="sm" variant="outline" disabled={saving} onClick={onCancel}>
          取消
        </Button>
        <Button type="submit" size="sm" disabled={saving}>
          {saving ? <Spinner /> : null}
          {isEditing ? "保存" : "添加"}
        </Button>
      </div>
    </form>
  );
}

export function McpImportForm({
  scope,
  json,
  projectId,
  projectName,
  importing,
  onScopeChange,
  onJsonChange,
  onCancel,
  onSubmit,
}: {
  scope: McpScope;
  json: string;
  projectId?: string | undefined;
  projectName?: string | undefined;
  importing: boolean;
  onScopeChange: (scope: McpScope) => void;
  onJsonChange: (value: string) => void;
  onCancel: () => void;
  onSubmit: (event: React.FormEvent) => void;
}) {
  return (
    <form onSubmit={onSubmit} className="flex flex-col gap-5">
      <div className="flex items-center gap-2">
        <Button type="button" size="sm" variant="ghost" className="-ms-2 text-muted-foreground hover:text-foreground" onClick={onCancel}>
          <ArrowLeft />
          返回
        </Button>
        <span className="text-sm font-medium text-foreground">导入 JSON</span>
      </div>
      <div className="flex flex-col gap-4 border-y border-border/60 py-4">
        <Field label="作用域">
          <Select
            value={scope}
            disabled={!projectId || importing}
            onValueChange={(value) => {
              if (value) onScopeChange(value as McpScope);
            }}
            items={[
              { value: "global", label: "全局" },
              ...(projectId ? [{ value: "project", label: projectName || "当前项目" }] : []),
            ]}
          >
            <SelectTrigger className="h-9 w-52">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="global">全局</SelectItem>
              {projectId ? <SelectItem value="project">{projectName || "当前项目"}</SelectItem> : null}
            </SelectContent>
          </Select>
        </Field>
        <Field label="JSON">
          <Textarea
            value={json}
            disabled={importing}
            rows={9}
            placeholder={'{\n  "mcpServers": {\n    "filesystem": {\n      "command": "npx",\n      "args": ["-y", "@modelcontextprotocol/server-filesystem", "."]\n    }\n  }\n}'}
            className="min-h-44 font-mono text-xs"
            onChange={(event) => onJsonChange(event.target.value)}
          />
        </Field>
      </div>
      <div className="flex items-center justify-end gap-2">
        <Button type="button" size="sm" variant="outline" disabled={importing} onClick={onCancel}>
          取消
        </Button>
        <Button type="submit" size="sm" disabled={importing || !json.trim()}>
          {importing ? <Spinner /> : <FileJson />}
          导入
        </Button>
      </div>
    </form>
  );
}

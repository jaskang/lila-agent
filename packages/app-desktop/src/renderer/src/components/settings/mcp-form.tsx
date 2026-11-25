import * as React from "react";
import { ArrowLeft, ChevronRight, FileJson, FolderOpen, Globe, Terminal } from "lucide-react";
import type { McpExposure, McpScope } from "@lila/protocol";
import { EXPOSURE_OPTIONS, type McpFormErrors, type McpFormState } from "@/components/settings/mcp-parse";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Field, FieldError, FieldGroup, FieldLabel, FieldLegend, FieldSet } from "@/components/ui/field";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "cn";

type FieldControlProps = { id: string; "aria-invalid"?: boolean; "aria-describedby"?: string };

function McpField({ label, error, children }: {
  label: string;
  error?: string | undefined;
  children: (props: FieldControlProps) => React.ReactNode;
}) {
  const id = React.useId();
  const errorId = `${id}-error`;
  return (
    <Field data-invalid={Boolean(error) || undefined} className="gap-1.5">
      <FieldLabel htmlFor={id} className="text-xs">{label}</FieldLabel>
      {children(error ? { id, "aria-invalid": true, "aria-describedby": errorId } : { id })}
      {error ? <FieldError id={errorId} className="whitespace-pre-line">{error}</FieldError> : null}
    </Field>
  );
}

export function McpServerForm({
  form,
  projectId,
  projectName,
  saving,
  errors,
  error,
  onChange,
  onCancel,
  onSubmit,
}: {
  form: McpFormState;
  projectId?: string | undefined;
  projectName?: string | undefined;
  saving: boolean;
  errors: McpFormErrors;
  error?: string | undefined;
  onChange: (next: McpFormState) => void;
  onCancel: () => void;
  onSubmit: (event: React.FormEvent) => void;
}) {
  const isEditing = Boolean(form.editingKey);
  const hasAdvanced = Boolean(
    form.cwd.trim() || form.envText.trim() || form.headersText.trim() || form.description.trim() || form.exposure !== "codemode",
  );
  const [advancedOpen, setAdvancedOpen] = React.useState(hasAdvanced);
  const hasAdvancedErrors = Boolean(errors.envText || errors.headersText);
  React.useEffect(() => {
    if (hasAdvancedErrors) setAdvancedOpen(true);
  }, [hasAdvancedErrors]);

  return (
    <form onSubmit={onSubmit} className="flex flex-col gap-5">
      <div className="flex items-center justify-between gap-2">
        <div className="flex min-w-0 items-center gap-2">
          <Button type="button" size="sm" variant="ghost" className="-ms-2 text-muted-foreground hover:text-foreground" onClick={onCancel}>
            <ArrowLeft />
            返回
          </Button>
          <span className="truncate text-sm font-medium text-foreground">{isEditing ? form.name : "添加 MCP"}</span>
          {isEditing ? <span className="truncate text-xs text-muted-foreground">{form.scope === "global" ? "全局" : projectName ?? "当前项目"}</span> : null}
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

      <FieldGroup className="gap-4 border-y border-border/60 py-4">
        {!isEditing ? (
          <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
            <McpField label="名称" error={errors.name}>
              {(control) => (
                <Input
                  {...control}
                  value={form.name}
                  disabled={saving}
                  placeholder="filesystem"
                  className="h-9 font-mono text-xs"
                  onChange={(event) => onChange({ ...form, name: event.target.value })}
                />
              )}
            </McpField>
            <McpField label="作用域">
              {(control) => (
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
                  <SelectTrigger {...control} className="h-9 w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="global">全局</SelectItem>
                    {projectId ? <SelectItem value="project">{projectName || "当前项目"}</SelectItem> : null}
                  </SelectContent>
                </Select>
              )}
            </McpField>
          </div>
        ) : null}

        <FieldSet className="gap-1.5">
          <FieldLegend className="mb-0 text-xs">连接方式</FieldLegend>
          <div className="inline-flex w-fit rounded-lg bg-muted/60 p-0.5">
            <Button
              type="button"
              size="xs"
              variant="ghost"
              className={cn(
                "h-7 gap-1.5 px-3 text-xs font-normal text-muted-foreground hover:text-foreground",
                form.transport === "stdio" && "bg-background font-medium text-foreground shadow-2xs",
              )}
              disabled={saving}
              aria-pressed={form.transport === "stdio"}
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
              disabled={saving}
              aria-pressed={form.transport === "http"}
              onClick={() => onChange({ ...form, transport: "http" })}
            >
              <Globe className="size-3.5" />
              HTTP
            </Button>
          </div>
        </FieldSet>

        {form.transport === "stdio" ? (
          <>
            <McpField label="命令" error={errors.command}>
              {(control) => (
                <Input
                  {...control}
                  value={form.command}
                  disabled={saving}
                  placeholder="npx"
                  className="h-9 font-mono text-xs"
                  onChange={(event) => onChange({ ...form, command: event.target.value })}
                />
              )}
            </McpField>
            <McpField label="参数">
              {(control) => (
                <Textarea
                  {...control}
                  value={form.argsText}
                  disabled={saving}
                  rows={2}
                  placeholder={"-y\n@modelcontextprotocol/server-filesystem\n."}
                  className="min-h-16 font-mono text-xs"
                  onChange={(event) => onChange({ ...form, argsText: event.target.value })}
                />
              )}
            </McpField>
          </>
        ) : (
          <McpField label="URL" error={errors.url}>
            {(control) => (
              <Input
                {...control}
                value={form.url}
                disabled={saving}
                placeholder="https://example.com/mcp"
                className="h-9 font-mono text-xs"
                onChange={(event) => onChange({ ...form, url: event.target.value })}
              />
            )}
          </McpField>
        )}
      </FieldGroup>

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
            <McpField label="备注">
              {(control) => (
                <Input
                  {...control}
                  value={form.description}
                  disabled={saving}
                  placeholder="简短备注"
                  className="h-9 text-xs"
                  onChange={(event) => onChange({ ...form, description: event.target.value })}
                />
              )}
            </McpField>
            <McpField label="暴露方式">
              {(control) => (
                <Select
                  value={form.exposure}
                  disabled={saving}
                  onValueChange={(value) => {
                    if (value) onChange({ ...form, exposure: value as McpExposure });
                  }}
                  items={EXPOSURE_OPTIONS}
                >
                  <SelectTrigger {...control} className="h-9 w-full">
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
              )}
            </McpField>
          </div>

          {form.transport === "stdio" ? (
            <>
              <McpField label="工作目录">
                {(control) => (
                  <div className="flex items-center gap-2">
                    <Input
                      {...control}
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
                )}
              </McpField>
              <McpField label="环境变量" error={errors.envText}>
                {(control) => (
                  <Textarea
                    {...control}
                    value={form.envText}
                    disabled={saving}
                    rows={3}
                    placeholder={"API_TOKEN=${MY_TOKEN}"}
                    className="min-h-20 font-mono text-xs"
                    onChange={(event) => onChange({ ...form, envText: event.target.value })}
                  />
                )}
              </McpField>
            </>
          ) : (
            <McpField label="请求头" error={errors.headersText}>
              {(control) => (
                <Textarea
                  {...control}
                  value={form.headersText}
                  disabled={saving}
                  rows={3}
                  placeholder={"Authorization: Bearer ${DOCS_TOKEN}"}
                  className="min-h-20 font-mono text-xs"
                  onChange={(event) => onChange({ ...form, headersText: event.target.value })}
                />
              )}
            </McpField>
          )}
        </CollapsibleContent>
      </Collapsible>

      {error ? <Alert variant="destructive"><AlertDescription>{error}</AlertDescription></Alert> : null}
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
  error,
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
  error?: string | undefined;
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
      <FieldGroup className="gap-4 border-y border-border/60 py-4">
        <McpField label="作用域">
          {(control) => (
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
              <SelectTrigger {...control} className="h-9 w-52">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="global">全局</SelectItem>
                {projectId ? <SelectItem value="project">{projectName || "当前项目"}</SelectItem> : null}
              </SelectContent>
            </Select>
          )}
        </McpField>
        <McpField label="JSON" error={error}>
          {(control) => (
            <Textarea
              {...control}
              value={json}
              disabled={importing}
              rows={9}
              placeholder={'{\n  "mcpServers": {\n    "filesystem": {\n      "command": "npx",\n      "args": ["-y", "@modelcontextprotocol/server-filesystem", "."]\n    }\n  }\n}'}
              className="min-h-44 font-mono text-xs"
              onChange={(event) => onJsonChange(event.target.value)}
            />
          )}
        </McpField>
      </FieldGroup>
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

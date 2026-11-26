import * as React from "react";
import { Check, CheckCircle2, Copy, ExternalLink, Eye, EyeOff, TriangleAlert } from "lucide-react";
import { toast } from "sonner";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";
import { cn } from "cn";
import { actions, type AuthLoginFlowState } from "@/lib/store";

export function LoginFlowCard({ flow, onDone }: { flow: AuthLoginFlowState; onDone: () => void }) {
  const [inputValue, setInputValue] = React.useState("");
  const [showSecret, setShowSecret] = React.useState(false);
  const [submitting, setSubmitting] = React.useState(false);
  const [copiedKey, setCopiedKey] = React.useState<string | undefined>();
  const openedUrlRef = React.useRef<string | undefined>(undefined);
  const inputRef = React.useRef<HTMLInputElement>(null);

  React.useEffect(() => {
    setInputValue("");
    setShowSecret(false);
    setSubmitting(false);
    if (flow.prompt && flow.prompt.type !== "select") {
      requestAnimationFrame(() => inputRef.current?.focus());
    }
  }, [flow.prompt]);

  React.useEffect(() => {
    const url = flow.authUrl?.url;
    if (!url || openedUrlRef.current === url) return;
    openedUrlRef.current = url;
    void actions.openExternalUrl(url);
    toast.info("已在浏览器中打开授权页面");
  }, [flow.authUrl?.url]);

  const copyText = (key: string, text: string) => {
    void navigator.clipboard
      .writeText(text)
      .then(() => {
        setCopiedKey(key);
        window.setTimeout(() => setCopiedKey((current) => (current === key ? undefined : current)), 1_500);
      })
      .catch(() => toast.error("复制失败"));
  };

  const submitValue = async (value: string) => {
    if (!flow.prompt || submitting) return;
    setSubmitting(true);
    try {
      await actions.respondProviderLogin(flow.prompt.promptId, value);
    } finally {
      setSubmitting(false);
    }
  };

  const isApiKey = flow.method === "api_key";

  return (
    <div className="flex flex-col gap-3 rounded-xl border border-border bg-muted/25 p-4">
      <div className="flex items-center justify-between gap-2">
        <div className="flex items-center gap-2">
          <span className="text-sm font-medium">{isApiKey ? `${flow.providerName} API Key` : `登录 ${flow.providerName}`}</span>
          {flow.status === "running" ? <Spinner className="size-3.5 text-muted-foreground" /> : null}
        </div>
        <Button
          type="button"
          size="xs"
          variant="ghost"
          onClick={() => {
            if (flow.status === "running") void actions.cancelProviderLogin();
            else {
              actions.clearAuthLogin();
              onDone();
            }
          }}
        >
          {flow.status === "running" ? "取消" : "关闭"}
        </Button>
      </div>

      {flow.authUrl ? (
        <div className="flex flex-col gap-2">
          <div className="flex flex-wrap items-center gap-2">
            <Button type="button" size="sm" onClick={() => void actions.openExternalUrl(flow.authUrl!.url)}>
              <ExternalLink />
              打开登录页
            </Button>
            <Button type="button" size="sm" variant="outline" onClick={() => copyText("auth-url", flow.authUrl!.url)}>
              <span className="swap-icon" aria-hidden="true">
                <Copy data-state={copiedKey === "auth-url" ? "hidden" : "visible"} />
                <Check data-state={copiedKey === "auth-url" ? "visible" : "hidden"} />
              </span>
              {copiedKey === "auth-url" ? "已复制" : "复制链接"}
            </Button>
          </div>
          {flow.authUrl.instructions ? <p className="text-xs text-muted-foreground">{flow.authUrl.instructions}</p> : null}
        </div>
      ) : null}

      {flow.deviceCode ? (
        <div className="flex flex-col gap-2 rounded-lg border border-border/70 bg-background p-3">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <span className="font-mono text-lg font-semibold tracking-widest text-foreground">{flow.deviceCode.userCode}</span>
            <div className="flex items-center gap-2">
              <Button type="button" size="sm" variant="outline" onClick={() => copyText("device-code", flow.deviceCode!.userCode)}>
                <span className="swap-icon" aria-hidden="true">
                  <Copy data-state={copiedKey === "device-code" ? "hidden" : "visible"} />
                  <Check data-state={copiedKey === "device-code" ? "visible" : "hidden"} />
                </span>
                {copiedKey === "device-code" ? "已复制" : "复制验证码"}
              </Button>
              <Button type="button" size="sm" onClick={() => void actions.openExternalUrl(flow.deviceCode!.verificationUri)}>
                <ExternalLink />
                打开验证页
              </Button>
            </div>
          </div>
          <span className="truncate font-mono text-xs text-muted-foreground">{flow.deviceCode.verificationUri}</span>
        </div>
      ) : null}

      {flow.info ? (
        <div className="flex flex-col gap-1.5 text-xs text-muted-foreground">
          <p className="whitespace-pre-wrap">{flow.info.message}</p>
          {flow.info.links?.length ? (
            <div className="flex flex-wrap gap-2">
              {flow.info.links.map((link) => (
                <button
                  key={link.url}
                  type="button"
                  onClick={() => void actions.openExternalUrl(link.url)}
                  className="inline-flex items-center gap-1 font-medium text-foreground underline underline-offset-3 hover:opacity-80"
                >
                  {link.label ?? link.url}
                  <ExternalLink className="size-3" />
                </button>
              ))}
            </div>
          ) : null}
        </div>
      ) : null}

      {flow.status === "running" && flow.prompt ? (
        flow.prompt.type === "select" ? (
          <div className="flex flex-col gap-2">
            <p className="text-xs font-medium text-foreground">{flow.prompt.message}</p>
            <div className="flex flex-col gap-1.5">
              {flow.prompt.options.map((option) => (
                <Button
                  key={option.id}
                  type="button"
                  variant="outline"
                  disabled={submitting}
                  className="h-auto justify-start px-3 py-2 text-left"
                  onClick={() => void submitValue(option.id)}
                >
                  <div className="flex min-w-0 flex-col gap-0.5">
                    <span className="text-xs font-medium text-foreground">{option.label}</span>
                    {option.description ? <span className="text-xs font-normal text-muted-foreground">{option.description}</span> : null}
                  </div>
                </Button>
              ))}
            </div>
          </div>
        ) : (
          <form
            className="flex flex-col gap-2"
            onSubmit={(event) => {
              event.preventDefault();
              void submitValue(inputValue.trim());
            }}
          >
            <label htmlFor="lila-auth-prompt-input" className="text-xs font-medium text-foreground">
              {flow.prompt.message}
            </label>
            <div className="flex items-center gap-2">
              <div className="relative min-w-0 flex-1">
                <Input
                  id="lila-auth-prompt-input"
                  ref={inputRef}
                  type={flow.prompt.type === "secret" && !showSecret ? "password" : "text"}
                  value={inputValue}
                  placeholder={flow.prompt.placeholder}
                  disabled={submitting}
                  className={cn("h-8 bg-background font-mono text-xs", flow.prompt.type === "secret" && "pr-8")}
                  onChange={(event) => setInputValue(event.target.value)}
                />
                {flow.prompt.type === "secret" ? (
                  <button
                    type="button"
                    aria-label={showSecret ? "隐藏内容" : "显示内容"}
                    onClick={() => setShowSecret((current) => !current)}
                    className="absolute top-1/2 right-2 -translate-y-1/2 rounded-sm p-0.5 text-muted-foreground transition-[color,background-color,scale] duration-150 ease-(--ease-ui-out) outline-none hover:bg-muted hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50 motion-safe:active:scale-[0.95]"
                  >
                    {showSecret ? <EyeOff className="size-3.5" /> : <Eye className="size-3.5" />}
                  </button>
                ) : null}
              </div>
              <Button type="submit" size="sm" disabled={submitting || !inputValue.trim()}>
                {submitting ? <Spinner /> : null}
                提交
              </Button>
            </div>
          </form>
        )
      ) : null}

      {flow.status === "running" && flow.progress ? (
        <div className="flex items-center gap-2 text-xs text-muted-foreground">
          <Spinner className="size-3.5" />
          <span>{flow.progress}</span>
        </div>
      ) : null}

      {flow.status === "done" ? (
        <div className="flex items-center justify-between gap-2 text-xs">
          <div className="flex items-center gap-2 text-success-foreground">
            <CheckCircle2 className="size-4 shrink-0" />
            <span>已连接 {flow.providerName}</span>
          </div>
          <Button
            type="button"
            size="xs"
            onClick={() => {
              actions.clearAuthLogin();
              onDone();
            }}
          >
            完成
          </Button>
        </div>
      ) : null}

      {flow.status === "error" ? (
        <Alert variant="destructive">
          <TriangleAlert />
          <AlertTitle>认证失败</AlertTitle>
          <AlertDescription>{flow.error ?? "未知错误"}</AlertDescription>
        </Alert>
      ) : null}
    </div>
  );
}

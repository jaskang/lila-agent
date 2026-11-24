import * as React from "react";
import { FileCode, Layers, Terminal } from "lucide-react";
import { Code } from "@/components/code";
import {
  Trace,
  TraceCollapsible,
  TraceCollapsibleContent,
  TraceCollapsibleTrigger,
  TraceDescription,
  TraceIcon,
  TraceLine,
  TraceTitle,
} from "@/components/ui/trace";
import { actions, useLila } from "@/lib/store";

export type StackFrame = {
  raw: string;
  name?: string | undefined;
  file?: string | undefined;
  line?: string | undefined;
  column?: string | undefined;
  internal: boolean;
};

export type ParsedStackTrace = {
  errorLine: string;
  appFrames: StackFrame[];
  internalFrames: StackFrame[];
};

const FRAME_WITH_FN = /^at\s+(.+?)\s+\((.+?):(\d+):(\d+)\)$/;
const FRAME_BARE = /^at\s+(.+?):(\d+):(\d+)$/;

function isInternalFrame(raw: string, file: string | undefined): boolean {
  const target = file ?? raw;
  return (
    target.includes("node_modules") ||
    target.startsWith("node:") ||
    target.startsWith("bun:") ||
    target.startsWith("internal/") ||
    target.includes("<anonymous>") ||
    raw.includes("(native)")
  );
}

function cleanFilePath(rawFile: string): string {
  return rawFile.startsWith("file://") ? rawFile.slice("file://".length) : rawFile;
}

function parseFrameLine(trimmed: string): StackFrame | undefined {
  const withFn = FRAME_WITH_FN.exec(trimmed);
  if (withFn) {
    const file = cleanFilePath(withFn[2]!);
    return {
      raw: trimmed,
      name: withFn[1],
      file,
      line: withFn[3],
      column: withFn[4],
      internal: isInternalFrame(trimmed, file),
    };
  }
  const bare = FRAME_BARE.exec(trimmed);
  if (bare) {
    const file = cleanFilePath(bare[1]!);
    return {
      raw: trimmed,
      file,
      line: bare[2],
      column: bare[3],
      internal: isInternalFrame(trimmed, file),
    };
  }
  if (trimmed.startsWith("at ")) {
    return { raw: trimmed, internal: isInternalFrame(trimmed, undefined) };
  }
  return undefined;
}

/** Detects a JS/Node/Bun stack trace inside tool output and splits app frames from framework internals. */
export function parseStackTrace(output: string): ParsedStackTrace | undefined {
  const lines = output.split(/\r?\n/);
  let firstFrameIndex = -1;
  const frames: StackFrame[] = [];

  for (let i = 0; i < lines.length; i++) {
    const trimmed = lines[i]!.trim();
    const frame = parseFrameLine(trimmed);
    if (frame) {
      if (firstFrameIndex === -1) firstFrameIndex = i;
      frames.push(frame);
    } else if (firstFrameIndex !== -1 && frames.length >= 2) {
      break;
    }
  }

  if (frames.length < 2 || firstFrameIndex <= 0) return undefined;

  let errorLine = "";
  for (let i = firstFrameIndex - 1; i >= 0; i--) {
    const candidate = lines[i]!.trim();
    if (candidate) {
      errorLine = candidate;
      break;
    }
  }
  if (!errorLine) return undefined;

  const appFrames = frames.filter((frame) => !frame.internal);
  const internalFrames = frames.filter((frame) => frame.internal);
  if (!appFrames.length && !internalFrames.length) return undefined;
  return { errorLine, appFrames, internalFrames };
}

function resolveProjectFile(projectPath: string | undefined, file: string | undefined): string | undefined {
  if (!file) return undefined;
  if (file.startsWith("/")) return file;
  if (!projectPath) return undefined;
  return `${projectPath.replace(/\/+$/, "")}/${file.replace(/^\.\//, "")}`;
}

function displayFilePath(file: string, projectPath: string | undefined): string {
  if (!projectPath) return file;
  const prefix = `${projectPath.replace(/\/+$/, "")}/`;
  return file.startsWith(prefix) ? file.slice(prefix.length) : file;
}

/** shadcn Stack Trace pattern: highlights app frames (clickable to open) and folds internal frames. */
export function StackTraceView({
  parsed,
  rawOutput,
  sessionId,
}: {
  parsed: ParsedStackTrace;
  rawOutput: string;
  sessionId: string;
}) {
  const projectPath = useLila((state) => {
    const projectId = state.sessions[sessionId]?.projectId;
    return state.projects.find((project) => project.id === projectId)?.path;
  });

  return (
    <div className="flex min-w-0 flex-col gap-2">
      <p className="rounded-lg border border-destructive/25 bg-destructive/8 px-3 py-2 font-mono text-xs break-words text-destructive-foreground">
        {parsed.errorLine}
      </p>
      <Trace className="gap-1.5">
        {parsed.appFrames.map((frame, index) => {
          const fullPath = resolveProjectFile(projectPath, frame.file);
          const shortFile = frame.file ? displayFilePath(frame.file, projectPath) : undefined;
          const location = shortFile ? (frame.line ? `${shortFile}:${frame.line}` : shortFile) : frame.raw;
          const lineContent = (
            <TraceLine className="text-xs">
              <TraceIcon>
                <FileCode />
              </TraceIcon>
              <TraceTitle className="font-mono text-foreground">{frame.name ?? location}</TraceTitle>
              {frame.name && shortFile ? (
                <TraceDescription className="truncate font-mono text-[11px]">{location}</TraceDescription>
              ) : null}
            </TraceLine>
          );
          if (!fullPath) return <div key={`${frame.raw}-${index}`}>{lineContent}</div>;
          const lineNum = frame.line ? Number(frame.line) : undefined;
          return (
            <button
              key={`${frame.raw}-${index}`}
              type="button"
              title={`预览 ${location}`}
              onClick={() => actions.openFilePreview(fullPath, lineNum ? { line: lineNum } : undefined)}
            >
              {lineContent}
            </button>
          );
        })}
        {parsed.internalFrames.length ? (
          <TraceCollapsible>
            <TraceCollapsibleTrigger>
              <TraceLine className="text-xs">
                <TraceIcon>
                  <Layers />
                </TraceIcon>
                <TraceTitle>{parsed.internalFrames.length} 条框架/内部调用帧</TraceTitle>
              </TraceLine>
            </TraceCollapsibleTrigger>
            <TraceCollapsibleContent>
              <ol className="flex flex-col gap-1 overflow-x-auto rounded-lg bg-muted p-2.5 font-mono text-[11px] text-muted-foreground">
                {parsed.internalFrames.map((frame, index) => (
                  <li key={`${frame.raw}-${index}`} className="whitespace-nowrap">
                    {frame.raw}
                  </li>
                ))}
              </ol>
            </TraceCollapsibleContent>
          </TraceCollapsible>
        ) : null}
        <TraceCollapsible>
          <TraceCollapsibleTrigger>
            <TraceLine className="text-xs">
              <TraceIcon>
                <Terminal />
              </TraceIcon>
              <TraceTitle>完整原始输出</TraceTitle>
            </TraceLine>
          </TraceCollapsibleTrigger>
          <TraceCollapsibleContent>
            <Code code={rawOutput} title="原始输出" maxHeight="18rem" />
          </TraceCollapsibleContent>
        </TraceCollapsible>
      </Trace>
    </div>
  );
}

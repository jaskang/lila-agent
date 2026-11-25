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
const FRAME_PYTHON = /^File\s+"(.+?)",\s+line\s+(\d+)(?:,\s+in\s+(.+))?$/;
const FRAME_TSC = /^(.+?)\((\d+),(\d+)\):\s+error\s+(TS\d+):\s+(.+)$/;
const FRAME_GO = /^([^\s]+\.go):(\d+)(?:\s+\+0x[0-9a-f]+)?$/;
const FRAME_RUST = /^at\s+([^\s]+\.rs):(\d+):(\d+)$/;

function isInternalFrame(raw: string, file: string | undefined): boolean {
  const target = file ?? raw;
  return (
    target.includes("node_modules") ||
    target.startsWith("node:") ||
    target.startsWith("bun:") ||
    target.startsWith("internal/") ||
    target.includes("/site-packages/") ||
    target.includes("/lib/python") ||
    target.includes("/usr/local/go/src/") ||
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
  const py = FRAME_PYTHON.exec(trimmed);
  if (py) {
    const file = cleanFilePath(py[1]!);
    return {
      raw: trimmed,
      name: py[3],
      file,
      line: py[2],
      internal: isInternalFrame(trimmed, file),
    };
  }
  const tsc = FRAME_TSC.exec(trimmed);
  if (tsc) {
    const file = cleanFilePath(tsc[1]!);
    return {
      raw: trimmed,
      name: `${tsc[4]}: ${tsc[5]}`,
      file,
      line: tsc[2],
      column: tsc[3],
      internal: isInternalFrame(trimmed, file),
    };
  }
  const go = FRAME_GO.exec(trimmed);
  if (go) {
    const file = cleanFilePath(go[1]!);
    return {
      raw: trimmed,
      file,
      line: go[2],
      internal: isInternalFrame(trimmed, file),
    };
  }
  const rust = FRAME_RUST.exec(trimmed);
  if (rust) {
    const file = cleanFilePath(rust[1]!);
    return {
      raw: trimmed,
      file,
      line: rust[2],
      column: rust[3],
      internal: isInternalFrame(trimmed, file),
    };
  }
  if (trimmed.startsWith("at ")) {
    return { raw: trimmed, internal: isInternalFrame(trimmed, undefined) };
  }
  return undefined;
}

/** Detects JS/Node/Bun, Python, TypeScript, Go, or Rust stack traces / compiler errors inside tool output. */
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
    } else if (firstFrameIndex !== -1 && frames.length >= 2 && trimmed === "") {
      break;
    }
  }

  if (frames.length < 1 || (frames.length === 1 && !frames[0]?.file)) return undefined;

  const headerLines: string[] = [];
  if (firstFrameIndex > 0) {
    for (let i = firstFrameIndex - 1; i >= 0 && headerLines.length < 4; i--) {
      const candidate = lines[i]!.trim();
      if (!candidate) {
        if (headerLines.length > 0) break;
        continue;
      }
      headerLines.unshift(candidate);
    }
  }
  // Python tracebacks put the exception message at the bottom.
  if (headerLines.length === 1 && headerLines[0]!.startsWith("Traceback")) {
    for (let i = lines.length - 1; i > firstFrameIndex; i--) {
      const tail = lines[i]!.trim();
      if (tail) {
        headerLines.push(tail);
        break;
      }
    }
  }
  const errorLine = headerLines.join("\n") || frames[0]?.name || frames[0]?.raw || "";
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
      <p className="rounded-lg border border-destructive/25 bg-destructive/8 px-3 py-2 font-mono text-xs whitespace-pre-wrap break-words text-destructive-foreground">
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

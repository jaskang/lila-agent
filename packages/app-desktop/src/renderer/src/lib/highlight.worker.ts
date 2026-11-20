/// <reference lib="webworker" />
import { createHighlighterCore, type HighlighterCore } from "shiki/core";
import { createJavaScriptRegexEngine } from "shiki/engine/javascript";

const languages = {
  bash: () => import("shiki/langs/shellscript.mjs"),
  c: () => import("shiki/langs/c.mjs"),
  cpp: () => import("shiki/langs/cpp.mjs"),
  csharp: () => import("shiki/langs/csharp.mjs"),
  css: () => import("shiki/langs/css.mjs"),
  dart: () => import("shiki/langs/dart.mjs"),
  diff: () => import("shiki/langs/diff.mjs"),
  dockerfile: () => import("shiki/langs/dockerfile.mjs"),
  go: () => import("shiki/langs/go.mjs"),
  graphql: () => import("shiki/langs/graphql.mjs"),
  html: () => import("shiki/langs/html.mjs"),
  ini: () => import("shiki/langs/ini.mjs"),
  java: () => import("shiki/langs/java.mjs"),
  javascript: () => import("shiki/langs/javascript.mjs"),
  json: () => import("shiki/langs/json.mjs"),
  jsonc: () => import("shiki/langs/jsonc.mjs"),
  jsx: () => import("shiki/langs/jsx.mjs"),
  kotlin: () => import("shiki/langs/kotlin.mjs"),
  lua: () => import("shiki/langs/lua.mjs"),
  markdown: () => import("shiki/langs/markdown.mjs"),
  "objective-c": () => import("shiki/langs/objective-c.mjs"),
  php: () => import("shiki/langs/php.mjs"),
  proto: () => import("shiki/langs/proto.mjs"),
  python: () => import("shiki/langs/python.mjs"),
  ruby: () => import("shiki/langs/ruby.mjs"),
  rust: () => import("shiki/langs/rust.mjs"),
  scss: () => import("shiki/langs/scss.mjs"),
  sql: () => import("shiki/langs/sql.mjs"),
  svelte: () => import("shiki/langs/svelte.mjs"),
  swift: () => import("shiki/langs/swift.mjs"),
  toml: () => import("shiki/langs/toml.mjs"),
  tsx: () => import("shiki/langs/tsx.mjs"),
  typescript: () => import("shiki/langs/typescript.mjs"),
  vue: () => import("shiki/langs/vue.mjs"),
  xml: () => import("shiki/langs/xml.mjs"),
  yaml: () => import("shiki/langs/yaml.mjs"),
};
type Language = keyof typeof languages;

const ALIASES: Record<string, Language> = {
  js: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  ts: "typescript",
  mts: "typescript",
  cts: "typescript",
  md: "markdown",
  mdx: "markdown",
  py: "python",
  sh: "bash",
  shell: "bash",
  shellscript: "bash",
  zsh: "bash",
  console: "bash",
  yml: "yaml",
  rs: "rust",
  h: "c",
  cc: "cpp",
  cxx: "cpp",
  "c++": "cpp",
  hpp: "cpp",
  hxx: "cpp",
  hh: "cpp",
  cs: "csharp",
  "c#": "csharp",
  kt: "kotlin",
  kts: "kotlin",
  rb: "ruby",
  docker: "dockerfile",
  gql: "graphql",
  svg: "xml",
  plist: "xml",
  xsl: "xml",
  objc: "objective-c",
  "obj-c": "objective-c",
  patch: "diff",
  protobuf: "proto",
  env: "ini",
  dotenv: "ini",
  conf: "ini",
  cfg: "ini",
  sass: "scss",
  less: "css",
};

export type HighlightRequest = { id: number; code: string; language: string };
/** One array per line; each token is its text plus light/dark colors. */
export type HighlightResponse = { id: number; lines: Array<Array<[string, string?, string?]>> | null };

let highlighter: Promise<HighlighterCore> | undefined;

async function tokenize(code: string, name: string): Promise<HighlightResponse["lines"]> {
  const language = name in languages ? (name as Language) : ALIASES[name];
  if (!language) return null;
  highlighter ??= createHighlighterCore({
    themes: [import("shiki/themes/github-light.mjs"), import("shiki/themes/github-dark.mjs")],
    langs: [],
    engine: createJavaScriptRegexEngine({ forgiving: true }),
  });
  const shiki = await highlighter;
  if (!shiki.getLoadedLanguages().includes(language)) await shiki.loadLanguage(languages[language]);
  const { tokens } = shiki.codeToTokens(code, {
    lang: language,
    themes: { light: "github-light", dark: "github-dark" },
    defaultColor: false,
  });
  return tokens.map((line) =>
    line.map((token) => {
      const style = token.htmlStyle as Record<string, string> | undefined;
      return [token.content, style?.["--shiki-light"], style?.["--shiki-dark"]];
    }),
  );
}

self.onmessage = (event: MessageEvent<HighlightRequest>) => {
  const { id, code, language } = event.data;
  void tokenize(code, language.toLowerCase())
    .catch(() => null)
    .then((lines) => self.postMessage({ id, lines } satisfies HighlightResponse));
};

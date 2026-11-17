/// <reference lib="webworker" />
import { createHighlighterCore, type HighlighterCore } from "shiki/core";
import { createJavaScriptRegexEngine } from "shiki/engine/javascript";

const languages = {
  bash: () => import("shiki/langs/shellscript.mjs"),
  css: () => import("shiki/langs/css.mjs"),
  html: () => import("shiki/langs/html.mjs"),
  javascript: () => import("shiki/langs/javascript.mjs"),
  json: () => import("shiki/langs/json.mjs"),
  jsx: () => import("shiki/langs/jsx.mjs"),
  markdown: () => import("shiki/langs/markdown.mjs"),
  python: () => import("shiki/langs/python.mjs"),
  sql: () => import("shiki/langs/sql.mjs"),
  tsx: () => import("shiki/langs/tsx.mjs"),
  typescript: () => import("shiki/langs/typescript.mjs"),
  yaml: () => import("shiki/langs/yaml.mjs"),
};
type Language = keyof typeof languages;

const ALIASES: Record<string, Language> = {
  js: "javascript",
  md: "markdown",
  py: "python",
  sh: "bash",
  shell: "bash",
  zsh: "bash",
  ts: "typescript",
  yml: "yaml",
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

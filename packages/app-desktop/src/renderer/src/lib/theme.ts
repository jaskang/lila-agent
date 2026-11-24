import * as React from "react";
import type { ThemeMode } from "../../../preload/index.ts";

export type { ThemeMode };
export type ResolvedTheme = "light" | "dark";

const THEME_STORAGE_KEY = "lila.theme";
const listeners = new Set<() => void>();

let initialized = false;
let currentMode: ThemeMode = "system";
let currentResolved: ResolvedTheme = "light";

function isThemeMode(value: string | null): value is ThemeMode {
  return value === "system" || value === "light" || value === "dark";
}

function readStoredTheme(): ThemeMode {
  try {
    const raw = localStorage.getItem(THEME_STORAGE_KEY);
    if (isThemeMode(raw)) return raw;
  } catch {
    // Ignore storage read errors.
  }
  return "system";
}

function systemPrefersDark(): boolean {
  return typeof window !== "undefined" && window.matchMedia("(prefers-color-scheme: dark)").matches;
}

function resolveTheme(mode: ThemeMode): ResolvedTheme {
  if (mode === "dark") return "dark";
  if (mode === "light") return "light";
  return systemPrefersDark() ? "dark" : "light";
}

function emit(): void {
  for (const listener of listeners) listener();
}

function applyDocumentTheme(mode: ThemeMode): void {
  currentResolved = resolveTheme(mode);
  if (typeof document !== "undefined") {
    document.documentElement.classList.toggle("dark", currentResolved === "dark");
  }
}

export function initTheme(): void {
  if (initialized) return;
  initialized = true;
  currentMode = readStoredTheme();
  applyDocumentTheme(currentMode);
  window.lila?.setTheme(currentMode);

  const media = window.matchMedia("(prefers-color-scheme: dark)");
  media.addEventListener("change", () => {
    if (currentMode !== "system") return;
    applyDocumentTheme("system");
    emit();
  });
}

export function setTheme(mode: ThemeMode): void {
  if (!initialized) initTheme();
  currentMode = mode;
  try {
    localStorage.setItem(THEME_STORAGE_KEY, mode);
  } catch {
    // Ignore storage write errors.
  }
  window.lila?.setTheme(mode);
  applyDocumentTheme(mode);
  emit();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function getModeSnapshot(): ThemeMode {
  return currentMode;
}

function getResolvedSnapshot(): ResolvedTheme {
  return currentResolved;
}

export function useTheme(): {
  theme: ThemeMode;
  resolvedTheme: ResolvedTheme;
  setTheme: (mode: ThemeMode) => void;
} {
  const theme = React.useSyncExternalStore(subscribe, getModeSnapshot, getModeSnapshot);
  const resolvedTheme = React.useSyncExternalStore(subscribe, getResolvedSnapshot, getResolvedSnapshot);
  return { theme, resolvedTheme, setTheme };
}

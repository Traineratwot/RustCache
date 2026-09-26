export type LangMode = "auto" | "ru" | "en";
export type ThemeMode = "auto" | "light" | "dark";
export type Lang = "ru" | "en";
export type Theme = "light" | "dark";

const LANG_KEY = "rustcache.lang";
const THEME_KEY = "rustcache.theme";

function readRaw(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writeRaw(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* ignore */
  }
}

export function loadLangMode(): LangMode {
  const v = readRaw(LANG_KEY);
  return v === "ru" || v === "en" || v === "auto" ? v : "auto";
}

export function saveLangMode(mode: LangMode): void {
  writeRaw(LANG_KEY, mode);
}

export function loadThemeMode(): ThemeMode {
  const v = readRaw(THEME_KEY);
  return v === "light" || v === "dark" || v === "auto" ? v : "auto";
}

export function saveThemeMode(mode: ThemeMode): void {
  writeRaw(THEME_KEY, mode);
}

export function resolveLang(mode: LangMode): Lang {
  if (mode === "ru" || mode === "en") return mode;
  const nav = (navigator.language || "en").toLowerCase();
  return nav.startsWith("ru") ? "ru" : "en";
}

export function resolveTheme(mode: ThemeMode): Theme {
  if (mode === "light" || mode === "dark") return mode;
  try {
    return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  } catch {
    return "light";
  }
}

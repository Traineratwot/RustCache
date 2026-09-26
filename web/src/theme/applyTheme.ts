import type { Theme, ThemeMode } from "../prefs/storage";
import { resolveTheme } from "../prefs/storage";

export function applyTheme(mode: ThemeMode): Theme {
  const theme = resolveTheme(mode);
  document.documentElement.setAttribute("data-theme", theme);
  const link = document.getElementById("pt-theme") as HTMLLinkElement | null;
  if (link) link.href = `/themes/lara-${theme}-indigo.css`;
  return theme;
}

export function watchSystemTheme(onChange: () => void): () => void {
  try {
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  } catch {
    return () => {};
  }
}

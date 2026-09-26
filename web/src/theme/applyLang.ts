import { i18n } from "../i18n";
import type { Lang, LangMode } from "../prefs/storage";
import { resolveLang } from "../prefs/storage";

export function applyLang(mode: LangMode): Lang {
  const lang = resolveLang(mode);
  document.documentElement.lang = lang;
  if (i18n.isInitialized && i18n.language !== lang) {
    i18n.changeLanguage(lang);
  }
  return lang;
}

import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
} from "react";
import { i18n } from "../i18n";
import { applyLang } from "../theme/applyLang";
import { applyTheme, watchSystemTheme } from "../theme/applyTheme";
import type { Lang, LangMode, Theme, ThemeMode } from "./storage";
import { loadLangMode, loadThemeMode, saveLangMode, saveThemeMode } from "./storage";

type PrefsValue = {
  langMode: LangMode;
  resolvedLang: Lang;
  setLangMode: (mode: LangMode) => void;
  themeMode: ThemeMode;
  resolvedTheme: Theme;
  setThemeMode: (mode: ThemeMode) => void;
};

const PrefsContext = createContext<PrefsValue | null>(null);

export function PrefsProvider({ children }: { children: ReactNode }) {
  const [langMode, setLangModeState] = useState<LangMode>(() => loadLangMode());
  const [themeMode, setThemeModeState] = useState<ThemeMode>(() => loadThemeMode());
  const [resolvedLang, setResolvedLang] = useState<Lang>(() => applyLang(loadLangMode()));
  const [resolvedTheme, setResolvedTheme] = useState<Theme>(() => applyTheme(loadThemeMode()));

  useEffect(() => {
    if (themeMode !== "auto") return;
    return watchSystemTheme(() => {
      setResolvedTheme(applyTheme("auto"));
    });
  }, [themeMode]);

  useEffect(() => {
    // keep i18n language in sync when resolvedLang changes from auto-detect
    if (i18n.isInitialized && i18n.language !== resolvedLang) {
      i18n.changeLanguage(resolvedLang);
    }
  }, [resolvedLang]);

  const setLangMode = useCallback((mode: LangMode) => {
    saveLangMode(mode);
    setLangModeState(mode);
    setResolvedLang(applyLang(mode));
  }, []);

  const setThemeMode = useCallback((mode: ThemeMode) => {
    saveThemeMode(mode);
    setThemeModeState(mode);
    setResolvedTheme(applyTheme(mode));
  }, []);

  const value = useMemo(
    () => ({
      langMode,
      resolvedLang,
      setLangMode,
      themeMode,
      resolvedTheme,
      setThemeMode,
    }),
    [langMode, resolvedLang, setLangMode, themeMode, resolvedTheme, setThemeMode],
  );

  return <PrefsContext.Provider value={value}>{children}</PrefsContext.Provider>;
}

export function usePrefs(): PrefsValue {
  const ctx = useContext(PrefsContext);
  if (!ctx) throw new Error("usePrefs must be used within PrefsProvider");
  return ctx;
}

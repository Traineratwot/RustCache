import { Button } from "primereact/button";
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { NavLink } from "react-router-dom";
import { usePrefs } from "../prefs/PrefsContext";
import type { LangMode, ThemeMode } from "../prefs/storage";

const navKeys: {
  to: string;
  icon: string;
  key: string;
  end?: boolean;
}[] = [
  { to: "/", icon: "pi pi-chart-bar", key: "dashboard", end: true },
  { to: "/health", icon: "pi pi-heart", key: "health" },
  { to: "/requests", icon: "pi pi-list", key: "requests" },
  { to: "/exclusions", icon: "pi pi-ban", key: "exclusions" },
  { to: "/cache", icon: "pi pi-database", key: "cache" },
  { to: "/connect", icon: "pi pi-link", key: "connect" },
  { to: "/settings", icon: "pi pi-cog", key: "settings" },
  { to: "/ca", icon: "pi pi-shield", key: "ca" },
];

const themeCycle: ThemeMode[] = ["auto", "light", "dark"];
const langCycle: LangMode[] = ["auto", "ru", "en"];

export default function Layout({ children }: { children: ReactNode }) {
  const { t } = useTranslation();
  const { themeMode, resolvedTheme, setThemeMode, langMode, resolvedLang, setLangMode } =
    usePrefs();

  const nextTheme = themeCycle[(themeCycle.indexOf(themeMode) + 1) % themeCycle.length];
  const nextLang = langCycle[(langCycle.indexOf(langMode) + 1) % langCycle.length];

  const themeIcon =
    themeMode === "auto" ? "pi pi-circle" : resolvedTheme === "dark" ? "pi pi-moon" : "pi pi-sun";

  const themeLabel =
    themeMode === "auto"
      ? t("ui.theme.auto")
      : themeMode === "dark"
        ? t("ui.theme.dark")
        : t("ui.theme.light");

  const langLabel = langMode === "auto" ? t("ui.lang.auto") : langMode === "ru" ? "RU" : "EN";

  return (
    <div className="app-shell">
      <aside className="app-sidebar">
        <div className="app-brand">
          <i className="pi pi-bolt" />
          <span>RustCache</span>
        </div>
        <nav className="app-nav">
          {navKeys.map((it) => (
            <NavLink key={it.to} to={it.to} end={it.end}>
              <i className={it.icon} />
              <span>{t(`nav.${it.key}`)}</span>
            </NavLink>
          ))}
        </nav>
        <div className="app-sidebar-footer">
          <Button
            icon={themeIcon}
            text
            size="small"
            tooltip={t("ui.theme.toggle")}
            tooltipOptions={{ position: "top" }}
            aria-label={t("ui.theme.toggle")}
            onClick={() => setThemeMode(nextTheme)}
            className="app-side-btn"
          />
          <span className="app-side-label" title={themeLabel}>
            {themeLabel}
          </span>
          <Button
            label={langLabel}
            text
            size="small"
            tooltip={t("ui.lang.toggle")}
            tooltipOptions={{ position: "top" }}
            aria-label={t("ui.lang.toggle")}
            onClick={() => setLangMode(nextLang)}
            className="app-side-btn"
          />
          <span className="app-side-label">{langMode === "auto" ? `(${resolvedLang})` : ""}</span>
        </div>
      </aside>
      <main className="app-main">{children}</main>
    </div>
  );
}

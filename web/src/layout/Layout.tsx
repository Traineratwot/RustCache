import { Dropdown } from "primereact/dropdown";
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

export default function Layout({ children }: { children: ReactNode }) {
  const { t } = useTranslation();
  const { themeMode, setThemeMode, langMode, resolvedLang, setLangMode } = usePrefs();

  const themeOptions = [
    { value: "auto" as ThemeMode, label: t("ui.theme.auto"), icon: "pi pi-circle" },
    { value: "light" as ThemeMode, label: t("ui.theme.light"), icon: "pi pi-sun" },
    { value: "dark" as ThemeMode, label: t("ui.theme.dark"), icon: "pi pi-moon" },
  ];

  const langOptions = [
    { value: "auto" as LangMode, label: `${t("ui.lang.auto")} (${resolvedLang})` },
    { value: "ru" as LangMode, label: t("ui.lang.ru") },
    { value: "en" as LangMode, label: t("ui.lang.en") },
  ];

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
          <Dropdown
            value={themeMode}
            options={themeOptions}
            optionLabel="label"
            optionValue="value"
            onChange={(e) => setThemeMode(e.value as ThemeMode)}
            className="app-side-dd"
            panelClassName="app-side-dd-panel"
            tooltip={t("ui.theme.label")}
            tooltipOptions={{ position: "top" }}
            itemTemplate={(opt) => (
              <span className="flex align-items-center gap-2">
                <i className={opt.icon} />
                {opt.label}
              </span>
            )}
            valueTemplate={(opt) => (
              <span className="flex align-items-center gap-2">
                <i
                  className={
                    opt?.value === "auto"
                      ? "pi pi-circle"
                      : opt?.value === "dark"
                        ? "pi pi-moon"
                        : "pi pi-sun"
                  }
                />
                {opt?.label ?? t("ui.theme.label")}
              </span>
            )}
          />
          <Dropdown
            value={langMode}
            options={langOptions}
            optionLabel="label"
            optionValue="value"
            onChange={(e) => setLangMode(e.value as LangMode)}
            className="app-side-dd"
            panelClassName="app-side-dd-panel"
            tooltip={t("ui.lang.label")}
            tooltipOptions={{ position: "top" }}
          />
        </div>
      </aside>
      <main className="app-main">{children}</main>
    </div>
  );
}

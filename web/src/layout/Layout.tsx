import type { ReactNode } from "react";
import { NavLink } from "react-router-dom";

const items = [
  { to: "/", icon: "pi pi-chart-bar", label: "Обзор", end: true },
  { to: "/health", icon: "pi pi-heart", label: "Состояние" },
  { to: "/requests", icon: "pi pi-list", label: "Запросы" },
  { to: "/exclusions", icon: "pi pi-ban", label: "Исключения" },
  { to: "/cache", icon: "pi pi-database", label: "Кеш" },
  { to: "/connect", icon: "pi pi-link", label: "Подключение" },
  { to: "/settings", icon: "pi pi-cog", label: "Настройки" },
  { to: "/ca", icon: "pi pi-shield", label: "Сертификат CA" },
];

export default function Layout({ children }: { children: ReactNode }) {
  return (
    <div className="app-shell">
      <aside className="app-sidebar">
        <div className="app-brand">
          <i className="pi pi-bolt" />
          <span>RustCache</span>
        </div>
        <nav className="app-nav">
          {items.map((it) => (
            <NavLink key={it.to} to={it.to} end={it.end}>
              <i className={it.icon} />
              <span>{it.label}</span>
            </NavLink>
          ))}
        </nav>
      </aside>
      <main className="app-main">{children}</main>
    </div>
  );
}

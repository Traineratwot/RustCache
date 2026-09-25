import { Badge } from "primereact/badge";
import { Button } from "primereact/button";
import { Card } from "primereact/card";
import { Column } from "primereact/column";
import { DataTable } from "primereact/datatable";
import { Tag } from "primereact/tag";
import { useCallback, useEffect, useState } from "react";
import { getHealth } from "../api/client";
import type { HealthInfo, ListenerInfo } from "../api/types";

function fmtUptime(s: number): string {
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  const parts: string[] = [];
  if (d > 0) parts.push(`${d} д`);
  if (h > 0) parts.push(`${h} ч`);
  if (m > 0) parts.push(`${m} мин`);
  parts.push(`${sec} с`);
  return parts.join(" ");
}

function RunningBadge({ running }: { running: boolean }) {
  return running ? (
    <Tag value="Работает" severity="success" icon="pi pi-check" />
  ) : (
    <Tag value="Не работает" severity="danger" icon="pi pi-times" />
  );
}

export default function Health() {
  const [health, setHealth] = useState<HealthInfo | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const h = await getHealth();
      setHealth(h);
      setError(null);
    } catch {
      setError("Не удалось получить состояние API");
    }
  }, []);

  useEffect(() => {
    load();
    const id = setInterval(load, 3000);
    return () => clearInterval(id);
  }, [load]);

  const listeners = health?.listeners ?? [];
  const allOk = listeners.every((l) => l.running);

  return (
    <>
      <div className="flex align-items-center gap-2 mb-3">
        <h1 className="page-title" style={{ marginBottom: 0 }}>
          Состояние
        </h1>
        {health && (
          <Badge
            value={allOk ? "Все слушатели OK" : "Есть проблемы"}
            severity={allOk ? "success" : "danger"}
            style={{ fontSize: "0.85rem" }}
          />
        )}
        <div className="flex-1" />
        <Button icon="pi pi-refresh" label="Обновить" text onClick={load} />
      </div>

      {error && <p style={{ color: "var(--red-500)" }}>{error}</p>}

      {health && (
        <div className="stat-grid mb-3">
          <div className="stat-card">
            <div className="label">Общее состояние</div>
            <div className="value accent">{health.ok ? "OK" : "Ошибка"}</div>
          </div>
          <div className="stat-card">
            <div className="label">Аптайм</div>
            <div className="value">{fmtUptime(health.uptime_s)}</div>
          </div>
          <div className="stat-card">
            <div className="label">Слушателей</div>
            <div className="value">
              {listeners.filter((l) => l.running).length} / {listeners.length}
            </div>
          </div>
          <div className="stat-card">
            <div className="label">Автоматическое обновление</div>
            <div className="value" style={{ fontSize: "1.1rem" }}>
              каждые 3 с
            </div>
          </div>
        </div>
      )}

      <Card title="Слушатели">
        <DataTable value={listeners} emptyMessage="Нет данных" size="small" stripedRows>
          <Column
            field="name"
            header="Служба"
            body={(l: ListenerInfo) => <span className="font-medium">{l.name}</span>}
          />
          <Column field="bind" header="Bind" body={(l: ListenerInfo) => <code>{l.bind}</code>} />
          <Column
            field="port"
            header="Порт"
            body={(l: ListenerInfo) => (
              <code style={{ fontSize: "1rem", fontWeight: 600 }}>{l.port}</code>
            )}
            style={{ width: "100px" }}
          />
          <Column
            field="running"
            header="Статус"
            body={(l: ListenerInfo) => <RunningBadge running={l.running} />}
            style={{ width: "160px" }}
          />
        </DataTable>
      </Card>

      <Card title="Порты по умолчанию" className="mt-3">
        <p style={{ marginTop: 0 }}>
          <Tag value="3128" severity="info" /> HTTP-прокси — кеширование обычного HTTP
          <br />
          <Tag value="3129" severity="info" /> HTTPS MITM — перехват HTTPS (требует CA)
          <br />
          <Tag value="1080" severity="secondary" /> SOCKS5 — туннель без кеша
          <br />
          <Tag value="8080" severity="success" /> REST API + этот интерфейс (только 127.0.0.1)
        </p>
      </Card>
    </>
  );
}

import { Button } from "primereact/button";
import { Card } from "primereact/card";
import { ConfirmDialog, confirmDialog } from "primereact/confirmdialog";
import { ProgressBar } from "primereact/progressbar";
import { Toast } from "primereact/toast";
import { useCallback, useEffect, useRef, useState } from "react";
import { getCache, getConfig, purgeCache } from "../api/client";
import type { CacheInfo, Config } from "../api/types";

function fmtBytes(n: number): string {
  if (n < 1024) return `${n} Б`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} КБ`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(2)} МБ`;
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} ГБ`;
}

export default function Cache() {
  const [info, setInfo] = useState<CacheInfo | null>(null);
  const [cfg, setCfg] = useState<Config | null>(null);
  const [busy, setBusy] = useState(false);
  const toast = useRef<Toast>(null);

  const load = useCallback(async () => {
    try {
      const [i, c] = await Promise.all([getCache(), getConfig()]);
      setInfo(i);
      setCfg(c);
    } catch {
      toast.current?.show({
        severity: "error",
        summary: "Ошибка",
        detail: "Не удалось загрузить данные кеша",
      });
    }
  }, []);

  useEffect(() => {
    load();
    const id = setInterval(load, 5000);
    return () => clearInterval(id);
  }, [load]);

  const maxBytes = cfg?.cache.max_bytes ?? 0;
  const used = info?.bytes ?? 0;
  const pct = maxBytes > 0 ? Math.min(100, Math.round((used / maxBytes) * 100)) : 0;

  const handlePurge = () => {
    confirmDialog({
      message: "Удалить все записи кеша? Действие необратимо.",
      header: "Очистка кеша",
      icon: "pi pi-exclamation-triangle",
      accept: async () => {
        setBusy(true);
        try {
          const r = await purgeCache();
          toast.current?.show({
            severity: "success",
            summary: "Готово",
            detail: `Удалено файлов: ${r.purged}`,
          });
          await load();
        } catch {
          toast.current?.show({
            severity: "error",
            summary: "Ошибка",
            detail: "Не удалось очистить кеш",
          });
        } finally {
          setBusy(false);
        }
      },
    });
  };

  return (
    <>
      <Toast ref={toast} />
      <ConfirmDialog />
      <h1 className="page-title">Кеш</h1>

      <div className="stat-grid">
        <div className="stat-card">
          <div className="label">Размер</div>
          <div className="value accent">{fmtBytes(used)}</div>
        </div>
        <div className="stat-card">
          <div className="label">Лимит</div>
          <div className="value">{maxBytes > 0 ? fmtBytes(maxBytes) : "—"}</div>
        </div>
        <div className="stat-card">
          <div className="label">Записей</div>
          <div className="value">{info ? String(info.entries) : "—"}</div>
        </div>
        <div className="stat-card">
          <div className="label">Использовано</div>
          <div className="value accent">{pct}%</div>
        </div>
      </div>

      <Card title="Заполнение" className="mt-3">
        <ProgressBar value={pct} showValue style={{ height: "14px" }} />
        <p className="mt-2">
          Директория кеша: <code>{cfg?.cache.dir ?? "—"}</code>
        </p>
      </Card>

      <Card title="Очистка" className="mt-3">
        <p style={{ marginTop: 0 }}>
          Полная очистка удаляет все закешированные ответы с диска. Ключи кеша не восстанавливаются
          — объекты будут запрошены у источника заново.
        </p>
        <Button
          label="Очистить весь кеш"
          icon="pi pi-trash"
          severity="danger"
          loading={busy}
          onClick={handlePurge}
        />
      </Card>
    </>
  );
}

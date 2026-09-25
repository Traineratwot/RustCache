import { Button } from "primereact/button";
import { Card } from "primereact/card";
import { InputNumber } from "primereact/inputnumber";
import { Message } from "primereact/message";
import { Toast } from "primereact/toast";
import { useCallback, useEffect, useRef, useState } from "react";
import { getConfig, reloadConfig, updateLogSettings } from "../api/client";
import type { Config, LogSettings } from "../api/types";

function Row({ label, value }: { label: string; value: string | number }) {
  return (
    <div className="flex justify-content-between align-items-center py-2 border-bottom-1 surface-border">
      <span className="text-color-secondary">{label}</span>
      <code style={{ fontSize: "0.95rem" }}>{value}</code>
    </div>
  );
}

export default function Settings() {
  const [cfg, setCfg] = useState<Config | null>(null);
  const [busy, setBusy] = useState(false);
  const [saveBusy, setSaveBusy] = useState(false);
  const [reloadMsg, setReloadMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const [logForm, setLogForm] = useState<LogSettings>({
    max_rows: 10000,
    max_age_days: 7,
    cleanup_interval_secs: 300,
  });
  const toast = useRef<Toast>(null);

  const load = useCallback(async () => {
    try {
      const c = await getConfig();
      setCfg(c);
      if (c.logs) {
        setLogForm({
          max_rows: c.logs.max_rows,
          max_age_days: c.logs.max_age_days,
          cleanup_interval_secs: c.logs.cleanup_interval_secs,
        });
      }
    } catch {
      toast.current?.show({
        severity: "error",
        summary: "Ошибка",
        detail: "Не удалось загрузить конфигурацию",
      });
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const handleReload = async () => {
    setBusy(true);
    try {
      const r = await reloadConfig();
      if (r.ok) {
        setReloadMsg({ ok: true, text: "Конфигурация перезагружена" });
        toast.current?.show({
          severity: "success",
          summary: "Готово",
          detail: "Конфигурация перезагружена",
        });
        if (r.config) setCfg(r.config);
        else await load();
      } else {
        setReloadMsg({ ok: false, text: r.error ?? "Ошибка перезагрузки" });
      }
    } catch {
      setReloadMsg({ ok: false, text: "Не удалось связаться с API" });
    } finally {
      setBusy(false);
    }
  };

  const handleSaveLogs = async () => {
    setSaveBusy(true);
    try {
      const r = await updateLogSettings(logForm);
      if (r.ok) {
        setLogForm(r.settings);
        toast.current?.show({
          severity: "success",
          summary: "Готово",
          detail: "Настройки журнала сохранены",
        });
      } else {
        toast.current?.show({
          severity: "error",
          summary: "Ошибка",
          detail: "Не удалось сохранить настройки",
        });
      }
    } catch {
      toast.current?.show({
        severity: "error",
        summary: "Ошибка",
        detail: "Не удалось сохранить настройки журнала",
      });
    } finally {
      setSaveBusy(false);
    }
  };

  return (
    <>
      <Toast ref={toast} />
      <h1 className="page-title">Настройки</h1>
      <Message
        severity="info"
        text="Параметры ниже — эффективная конфигурация. Настройки журнала редактируются; остальное только чтение. Hot-reload применяет изменения исключений; порты и лимиты кеша требуют перезапуска."
        className="mb-3 w-full"
      />

      {!cfg ? (
        <p>Загрузка...</p>
      ) : (
        <div className="grid">
          <div className="col-12 md:col-6">
            <Card title="Слушатели">
              <Row label="HTTP proxy" value={cfg.http.port} />
              <Row label="HTTPS MITM" value={cfg.https.port} />
              <Row label="SOCKS5" value={cfg.socks5.port} />
              <Row label="API bind" value={cfg.api.bind} />
            </Card>
          </div>
          <div className="col-12 md:col-6">
            <Card title="Пути">
              <Row label="Данные (data_dir)" value={cfg.data_dir} />
              <Row label="Кеш" value={cfg.cache.dir} />
              <Row label="CA" value={cfg.ca.dir} />
              <Row label="Журнал" value={cfg.logs.db_path} />
              <p className="text-color-secondary" style={{ marginBottom: 0, fontSize: "0.9rem" }}>
                Относительные пути разрешаются от data_dir. Абсолютные используются как есть. CLI:{" "}
                <code>--data-dir</code>. Смена каталога — через config.toml или флаг, нужен
                перезапуск.
              </p>
            </Card>
          </div>
          <div className="col-12 md:col-6">
            <Card title="Кеш">
              <Row label="Директория" value={cfg.cache.dir} />
              <Row
                label="Макс. размер"
                value={`${(cfg.cache.max_bytes / 1024 / 1024).toFixed(0)} МБ`}
              />
              <Row
                label="Макс. объект"
                value={`${(cfg.cache.max_object_bytes / 1024 / 1024).toFixed(0)} МБ`}
              />
            </Card>
          </div>
          <div className="col-12 md:col-6">
            <Card title="CA">
              <Row label="Директория" value={cfg.ca.dir} />
            </Card>
          </div>
          <div className="col-12 md:col-6">
            <Card title="Hot-reload">
              <p style={{ marginTop: 0 }}>
                Перечитать config.toml с диска и применить изменения исключений без перезапуска.
              </p>
              <Button
                label="Reload config"
                icon="pi pi-refresh"
                loading={busy}
                onClick={handleReload}
              />
              {reloadMsg && (
                <Message
                  severity={reloadMsg.ok ? "success" : "error"}
                  text={reloadMsg.text}
                  className="mt-2 w-full"
                />
              )}
            </Card>
          </div>
          <div className="col-12 md:col-6">
            <Card title="Логи запросов">
              <div className="flex flex-column gap-3">
                <div className="flex flex-column gap-1">
                  <label htmlFor="log-max-rows" className="text-color-secondary">
                    Максимум записей
                  </label>
                  <InputNumber
                    id="log-max-rows"
                    value={logForm.max_rows}
                    onValueChange={(e) =>
                      setLogForm((f) => ({ ...f, max_rows: e.value ?? f.max_rows }))
                    }
                    min={1}
                    max={10_000_000}
                    showButtons
                  />
                </div>
                <div className="flex flex-column gap-1">
                  <label htmlFor="log-max-age" className="text-color-secondary">
                    Хранить дней
                  </label>
                  <InputNumber
                    id="log-max-age"
                    value={logForm.max_age_days}
                    onValueChange={(e) =>
                      setLogForm((f) => ({ ...f, max_age_days: e.value ?? f.max_age_days }))
                    }
                    min={1}
                    max={3650}
                    showButtons
                  />
                </div>
                <div className="flex flex-column gap-1">
                  <label htmlFor="log-interval" className="text-color-secondary">
                    Интервал очистки, сек
                  </label>
                  <InputNumber
                    id="log-interval"
                    value={logForm.cleanup_interval_secs}
                    onValueChange={(e) =>
                      setLogForm((f) => ({
                        ...f,
                        cleanup_interval_secs: e.value ?? f.cleanup_interval_secs,
                      }))
                    }
                    min={10}
                    max={86400}
                    showButtons
                  />
                </div>
                <Button
                  label="Сохранить"
                  icon="pi pi-save"
                  loading={saveBusy}
                  onClick={handleSaveLogs}
                />
              </div>
            </Card>
          </div>
        </div>
      )}
    </>
  );
}

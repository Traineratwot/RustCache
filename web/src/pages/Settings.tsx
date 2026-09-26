import { Button } from "primereact/button";
import { Card } from "primereact/card";
import { InputNumber } from "primereact/inputnumber";
import { Message } from "primereact/message";
import { RadioButton } from "primereact/radiobutton";
import { Toast } from "primereact/toast";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { getConfig, reloadConfig, updateLogSettings } from "../api/client";
import type { Config, LogSettings } from "../api/types";
import { fmtMb } from "../lib/format";
import { usePrefs } from "../prefs/PrefsContext";
import type { LangMode, ThemeMode } from "../prefs/storage";

function Row({ label, value }: { label: string; value: string | number }) {
  return (
    <div className="flex justify-content-between align-items-center py-2 border-bottom-1 surface-border">
      <span className="text-color-secondary">{label}</span>
      <code style={{ fontSize: "0.95rem" }}>{value}</code>
    </div>
  );
}

const themeOptions: { key: ThemeMode; labelKey: string }[] = [
  { key: "auto", labelKey: "ui.theme.auto" },
  { key: "light", labelKey: "ui.theme.light" },
  { key: "dark", labelKey: "ui.theme.dark" },
];

const langOptions: { key: LangMode; labelKey: string }[] = [
  { key: "auto", labelKey: "ui.lang.auto" },
  { key: "ru", labelKey: "ui.lang.ru" },
  { key: "en", labelKey: "ui.lang.en" },
];

export default function Settings() {
  const { t } = useTranslation();
  const { langMode, setLangMode, themeMode, setThemeMode } = usePrefs();
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
        summary: t("common.error"),
        detail: t("settings.loadError"),
      });
    }
  }, [t]);

  useEffect(() => {
    load();
  }, [load]);

  const handleReload = async () => {
    setBusy(true);
    try {
      const r = await reloadConfig();
      if (r.ok) {
        setReloadMsg({ ok: true, text: t("settings.reloaded") });
        toast.current?.show({
          severity: "success",
          summary: t("common.done"),
          detail: t("settings.reloaded"),
        });
        if (r.config) setCfg(r.config);
        else await load();
      } else {
        setReloadMsg({ ok: false, text: r.error ?? t("settings.reloadError") });
      }
    } catch {
      setReloadMsg({ ok: false, text: t("settings.reloadApiError") });
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
          summary: t("common.done"),
          detail: t("settings.logsSaved"),
        });
      } else {
        toast.current?.show({
          severity: "error",
          summary: t("common.error"),
          detail: t("settings.logsSaveError"),
        });
      }
    } catch {
      toast.current?.show({
        severity: "error",
        summary: t("common.error"),
        detail: t("settings.logsSaveError2"),
      });
    } finally {
      setSaveBusy(false);
    }
  };

  return (
    <>
      <Toast ref={toast} />
      <h1 className="page-title">{t("settings.title")}</h1>
      <Message severity="info" text={t("settings.intro")} className="mb-3 w-full" />

      {!cfg ? (
        <p>{t("common.loading")}</p>
      ) : (
        <div className="grid">
          <div className="col-12 md:col-6">
            <Card title={t("ui.section")}>
              <div className="flex flex-column gap-3">
                <div>
                  <div className="text-color-secondary mb-2">{t("ui.lang.label")}</div>
                  <div className="flex flex-wrap gap-3">
                    {langOptions.map((o) => (
                      <div key={o.key} className="flex align-items-center gap-2">
                        <RadioButton
                          inputId={`lang-${o.key}`}
                          name="lang"
                          value={o.key}
                          onChange={() => setLangMode(o.key)}
                          checked={langMode === o.key}
                        />
                        <label htmlFor={`lang-${o.key}`}>{t(o.labelKey)}</label>
                      </div>
                    ))}
                  </div>
                </div>
                <div>
                  <div className="text-color-secondary mb-2">{t("ui.theme.label")}</div>
                  <div className="flex flex-wrap gap-3">
                    {themeOptions.map((o) => (
                      <div key={o.key} className="flex align-items-center gap-2">
                        <RadioButton
                          inputId={`theme-${o.key}`}
                          name="theme"
                          value={o.key}
                          onChange={() => setThemeMode(o.key)}
                          checked={themeMode === o.key}
                        />
                        <label htmlFor={`theme-${o.key}`}>{t(o.labelKey)}</label>
                      </div>
                    ))}
                  </div>
                </div>
              </div>
            </Card>
          </div>
          <div className="col-12 md:col-6">
            <Card title={t("settings.listeners")}>
              <Row label={t("settings.httpProxy")} value={cfg.http.port} />
              <Row label={t("settings.httpsMitm")} value={cfg.https.port} />
              <Row label={t("settings.socks5")} value={cfg.socks5.port} />
              <Row label={t("settings.apiBind")} value={cfg.api.bind} />
            </Card>
          </div>
          <div className="col-12 md:col-6">
            <Card title={t("settings.paths")}>
              <Row label={t("settings.dataDir")} value={cfg.data_dir} />
              <Row label={t("settings.cacheDir")} value={cfg.cache.dir} />
              <Row label={t("settings.caDir")} value={cfg.ca.dir} />
              <Row label={t("settings.logDb")} value={cfg.logs.db_path} />
              <p className="text-color-secondary" style={{ marginBottom: 0, fontSize: "0.9rem" }}>
                {t("settings.pathsHint")}
              </p>
            </Card>
          </div>
          <div className="col-12 md:col-6">
            <Card title={t("settings.cacheSection")}>
              <Row label={t("settings.dir")} value={cfg.cache.dir} />
              <Row label={t("settings.maxSize")} value={fmtMb(cfg.cache.max_bytes)} />
              <Row label={t("settings.maxObject")} value={fmtMb(cfg.cache.max_object_bytes)} />
            </Card>
          </div>
          <div className="col-12 md:col-6">
            <Card title={t("settings.caSection")}>
              <Row label={t("settings.dir")} value={cfg.ca.dir} />
            </Card>
          </div>
          <div className="col-12 md:col-6">
            <Card title={t("settings.hotReload")}>
              <p style={{ marginTop: 0 }}>{t("settings.hotReloadDesc")}</p>
              <Button
                label={t("settings.reload")}
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
            <Card title={t("settings.logs")}>
              <div className="flex flex-column gap-3">
                <div className="flex flex-column gap-1">
                  <label htmlFor="log-max-rows" className="text-color-secondary">
                    {t("settings.maxRows")}
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
                    {t("settings.maxAge")}
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
                    {t("settings.cleanupInterval")}
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
                  label={t("common.save")}
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

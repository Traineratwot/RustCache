import { Button } from "primereact/button";
import { Card } from "primereact/card";
import { ConfirmDialog, confirmDialog } from "primereact/confirmdialog";
import { Dropdown } from "primereact/dropdown";
import { InputNumber } from "primereact/inputnumber";
import { InputSwitch } from "primereact/inputswitch";
import { InputText } from "primereact/inputtext";
import { Message } from "primereact/message";
import { Toast } from "primereact/toast";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { getConfig, getHealth, reloadConfig, restartProcess, updateConfig } from "../api/client";
import type { Config, PacMode } from "../api/types";
import { Field, PathResolved, SectionTitle } from "../components/settings/fields";
import {
  MAX_CLEANUP_INTERVAL_SECS,
  MAX_LOG_AGE_DAYS,
  MAX_LOG_ROWS,
  MIB,
  RESTART_POLL_MS,
} from "../lib/constants";

type Form = Config;

const PAC_MODES: { labelKey: string; value: PacMode }[] = [
  { labelKey: "settings.pacModeHttp", value: "http" },
  { labelKey: "settings.pacModeSocks", value: "socks" },
  { labelKey: "settings.pacModeBoth", value: "http+socks" },
];

function cloneForm(c: Form): Form {
  return JSON.parse(JSON.stringify(c)) as Form;
}

/** Settings: edit every `config.toml` field with validation and hot/restart apply. */
export default function Settings() {
  const { t } = useTranslation();
  const [form, setForm] = useState<Form | null>(null);
  const [original, setOriginal] = useState<Form | null>(null);
  const [busy, setBusy] = useState(false);
  const [saveBusy, setSaveBusy] = useState(false);
  const [restartBusy, setRestartBusy] = useState(false);
  const [reloadMsg, setReloadMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const [restartFields, setRestartFields] = useState<string[]>([]);
  const [fieldErrors, setFieldErrors] = useState<Record<string, string>>({});
  const toast = useRef<Toast>(null);

  const dirty =
    form !== null && original !== null && JSON.stringify(form) !== JSON.stringify(original);

  const load = useCallback(async () => {
    try {
      const c = await getConfig();
      setForm(c);
      setOriginal(cloneForm(c));
      setFieldErrors({});
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

  /** Patch the form. Does not wipe validation errors — `save` resets them. */
  const patch = (fn: (f: Form) => Form) => {
    setForm((prev) => (prev ? fn(prev) : prev));
  };

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
        if (r.config) {
          setForm(r.config);
          setOriginal(cloneForm(r.config));
        } else {
          await load();
        }
      } else {
        setReloadMsg({ ok: false, text: r.error ?? t("settings.reloadError") });
      }
    } catch {
      setReloadMsg({ ok: false, text: t("settings.reloadApiError") });
    } finally {
      setBusy(false);
    }
  };

  const handleSave = async () => {
    if (!form) return;
    setSaveBusy(true);
    setFieldErrors({});
    try {
      const r = await updateConfig(form);
      if (r.ok && r.config) {
        setForm(r.config);
        setOriginal(cloneForm(r.config));
        setRestartFields(r.restart_fields ?? []);
        setFieldErrors({});
        if (r.restart_required) {
          toast.current?.show({
            severity: "warn",
            summary: t("common.warning"),
            detail: t("settings.savedRestart", {
              fields: (r.restart_fields ?? []).join(", "),
            }),
            life: 8000,
          });
        } else {
          toast.current?.show({
            severity: "success",
            summary: t("common.done"),
            detail: t("settings.saved"),
          });
        }
      } else {
        const map: Record<string, string> = {};
        for (const e of r.errors ?? []) map[e.field] = e.message;
        setFieldErrors(map);
        toast.current?.show({
          severity: "error",
          summary: t("common.error"),
          detail: r.error ?? t("settings.saveError"),
          life: 8000,
        });
      }
    } catch {
      toast.current?.show({
        severity: "error",
        summary: t("common.error"),
        detail: t("settings.saveError2"),
      });
    } finally {
      setSaveBusy(false);
    }
  };

  const waitUntilUp = useCallback(async () => {
    for (let i = 0; i < 30; i++) {
      await new Promise((r) => setTimeout(r, RESTART_POLL_MS));
      try {
        await getHealth();
        toast.current?.show({
          severity: "success",
          summary: t("common.done"),
          detail: t("settings.restarted"),
        });
        setRestartFields([]);
        await load();
        return;
      } catch {
        // still down
      }
    }
    toast.current?.show({
      severity: "error",
      summary: t("common.error"),
      detail: t("settings.restartError"),
      life: 8000,
    });
  }, [load, t]);

  const handleRestart = useCallback(async () => {
    setRestartBusy(true);
    try {
      await restartProcess();
      toast.current?.show({
        severity: "info",
        summary: t("settings.restarting"),
        life: 4000,
      });
    } catch {
      // The process may exit before the response is delivered.
      toast.current?.show({
        severity: "info",
        summary: t("settings.restarting"),
        life: 4000,
      });
    }
    await waitUntilUp();
    setRestartBusy(false);
  }, [t, waitUntilUp]);

  const handleRestartClick = () => {
    confirmDialog({
      message: t("settings.restartConfirm"),
      header: t("settings.restartTitle"),
      icon: "pi pi-exclamation-triangle",
      acceptLabel: t("settings.restartAccept"),
      rejectLabel: t("common.cancel"),
      accept: handleRestart,
    });
  };

  if (!form) {
    return (
      <>
        <Toast ref={toast} />
        <h1 className="page-title">{t("settings.title")}</h1>
        <p>{t("common.loading")}</p>
      </>
    );
  }

  return (
    <>
      <Toast ref={toast} />
      <ConfirmDialog />
      <h1 className="page-title">{t("settings.title")}</h1>
      <Message severity="info" text={t("settings.intro")} className="mb-3 w-full" />

      {restartFields.length > 0 && (
        <Message
          severity="warn"
          text={t("settings.restartBanner", { fields: restartFields.join(", ") })}
          className="mb-3 w-full"
        />
      )}

      <div className="settings-grid">
        {/* Listeners */}
        <Card>
          <SectionTitle>{t("settings.listeners")}</SectionTitle>
          <Field
            label={t("settings.httpProxy")}
            hint={t("settings.httpProxyHint")}
            apply="restart"
            error={fieldErrors["http.port"]}
          >
            <InputNumber
              value={form.http.port}
              min={1}
              max={65535}
              showButtons
              onValueChange={(e) =>
                patch((f) => ({ ...f, http: { port: e.value ?? f.http.port } }))
              }
            />
          </Field>
          <Field
            label={t("settings.httpsMitm")}
            hint={t("settings.httpsMitmHint")}
            apply="restart"
            error={fieldErrors["https.port"]}
          >
            <InputNumber
              value={form.https.port}
              min={1}
              max={65535}
              showButtons
              onValueChange={(e) =>
                patch((f) => ({ ...f, https: { port: e.value ?? f.https.port } }))
              }
            />
          </Field>
          <Field
            label={t("settings.socks5")}
            hint={t("settings.socks5Hint")}
            apply="restart"
            error={fieldErrors["socks5.port"]}
          >
            <InputNumber
              value={form.socks5.port}
              min={1}
              max={65535}
              showButtons
              onValueChange={(e) =>
                patch((f) => ({ ...f, socks5: { port: e.value ?? f.socks5.port } }))
              }
            />
          </Field>
          <Field
            label={t("settings.apiBind")}
            hint={t("settings.apiBindHint")}
            apply="restart"
            error={fieldErrors["api.bind"]}
          >
            <InputText
              value={form.api.bind}
              className="w-full"
              onChange={(e) => patch((f) => ({ ...f, api: { bind: e.target.value } }))}
            />
          </Field>
        </Card>

        {/* Cache limits */}
        <Card>
          <SectionTitle>{t("settings.cacheSection")}</SectionTitle>
          <Field
            label={t("settings.maxSize")}
            hint={t("settings.maxSizeHint")}
            apply="hot"
            error={fieldErrors["cache.max_bytes"]}
          >
            <InputNumber
              value={Math.round(form.cache.max_bytes / MIB)}
              min={1}
              max={1_048_576}
              suffix=" MB"
              showButtons
              onValueChange={(e) =>
                patch((f) => ({
                  ...f,
                  cache: {
                    ...f.cache,
                    max_bytes: Math.max(1, e.value ?? 1) * MIB,
                  },
                }))
              }
            />
          </Field>
          <Field
            label={t("settings.maxObject")}
            hint={t("settings.maxObjectHint")}
            apply="hot"
            error={fieldErrors["cache.max_object_bytes"]}
          >
            <InputNumber
              value={Math.round(form.cache.max_object_bytes / MIB)}
              min={1}
              max={1_048_576}
              suffix=" MB"
              showButtons
              onValueChange={(e) =>
                patch((f) => ({
                  ...f,
                  cache: {
                    ...f.cache,
                    max_object_bytes: Math.max(1, e.value ?? 1) * MIB,
                  },
                }))
              }
            />
          </Field>
          <p className="text-color-secondary mb-0" style={{ fontSize: "0.85rem" }}>
            {t("settings.cacheDirLabel")}: <code>{form.cache.dir}</code>
            <PathResolved dataDir={form.data_dir} value={form.cache.dir} />
          </p>
        </Card>

        {/* PAC */}
        <Card>
          <SectionTitle>{t("settings.pacSection")}</SectionTitle>
          <Field
            label={t("settings.pacEnabled")}
            hint={t("settings.pacEnabledHint")}
            apply="restart"
            error={fieldErrors["pac.enabled"]}
          >
            <InputSwitch
              checked={form.pac.enabled}
              onChange={(e) => patch((f) => ({ ...f, pac: { ...f.pac, enabled: e.value } }))}
            />
          </Field>
          <Field
            label={t("settings.pacBind")}
            hint={t("settings.pacBindHint")}
            apply="restart"
            error={fieldErrors["pac.bind"]}
          >
            <InputText
              value={form.pac.bind}
              className="w-full"
              disabled={!form.pac.enabled}
              onChange={(e) => patch((f) => ({ ...f, pac: { ...f.pac, bind: e.target.value } }))}
            />
          </Field>
          <Field
            label={t("settings.pacMode")}
            hint={t("settings.pacModeHint")}
            apply="hot"
            error={fieldErrors["pac.mode"]}
          >
            <Dropdown
              value={form.pac.mode}
              options={PAC_MODES.map((m) => ({ label: t(m.labelKey), value: m.value }))}
              onChange={(e) => patch((f) => ({ ...f, pac: { ...f.pac, mode: e.value } }))}
              className="w-full"
              disabled={!form.pac.enabled}
            />
          </Field>
        </Card>

        {/* Request logs */}
        <Card>
          <SectionTitle>{t("settings.logs")}</SectionTitle>
          <Field label={t("settings.maxRows")} apply="hot" error={fieldErrors["logs.max_rows"]}>
            <InputNumber
              value={form.logs.max_rows}
              min={1}
              max={MAX_LOG_ROWS}
              showButtons
              onValueChange={(e) =>
                patch((f) => ({
                  ...f,
                  logs: { ...f.logs, max_rows: e.value ?? f.logs.max_rows },
                }))
              }
            />
          </Field>
          <Field label={t("settings.maxAge")} apply="hot" error={fieldErrors["logs.max_age_days"]}>
            <InputNumber
              value={form.logs.max_age_days}
              min={1}
              max={MAX_LOG_AGE_DAYS}
              showButtons
              onValueChange={(e) =>
                patch((f) => ({
                  ...f,
                  logs: { ...f.logs, max_age_days: e.value ?? f.logs.max_age_days },
                }))
              }
            />
          </Field>
          <Field
            label={t("settings.cleanupInterval")}
            apply="hot"
            error={fieldErrors["logs.cleanup_interval_secs"]}
          >
            <InputNumber
              value={form.logs.cleanup_interval_secs}
              min={10}
              max={MAX_CLEANUP_INTERVAL_SECS}
              showButtons
              onValueChange={(e) =>
                patch((f) => ({
                  ...f,
                  logs: {
                    ...f.logs,
                    cleanup_interval_secs: e.value ?? f.logs.cleanup_interval_secs,
                  },
                }))
              }
            />
          </Field>
          <p className="text-color-secondary mb-0" style={{ fontSize: "0.85rem" }}>
            {t("settings.logDb")}: <code>{form.logs.db_path}</code>
            <PathResolved dataDir={form.data_dir} value={form.logs.db_path} />
          </p>
        </Card>

        {/* Paths: data_dir is the single root; the rest are sub-paths under it */}
        <Card>
          <SectionTitle>{t("settings.paths")}</SectionTitle>
          <p className="text-color-secondary mt-0" style={{ fontSize: "0.85rem" }}>
            {t("settings.pathsModel")}
          </p>
          <Field
            label={t("settings.dataDir")}
            hint={t("settings.dataDirHint")}
            apply="restart"
            error={fieldErrors.data_dir}
          >
            <InputText
              value={form.data_dir}
              className="w-full"
              onChange={(e) => patch((f) => ({ ...f, data_dir: e.target.value }))}
            />
            <span className="path-resolved">
              <span className="tag">{t("settings.pathRoot")}</span>
              <code>{form.data_dir.trim() || "—"}</code>
            </span>
          </Field>

          <div className="path-tree">
            <Field
              label={t("settings.cacheDir")}
              hint={t("settings.cacheDirHint")}
              apply="restart"
              error={fieldErrors["cache.dir"]}
            >
              <InputText
                value={form.cache.dir}
                className="w-full"
                onChange={(e) =>
                  patch((f) => ({ ...f, cache: { ...f.cache, dir: e.target.value } }))
                }
              />
              <PathResolved dataDir={form.data_dir} value={form.cache.dir} />
            </Field>
            <Field
              label={t("settings.caDir")}
              hint={t("settings.caDirHint")}
              apply="restart"
              error={fieldErrors["ca.dir"]}
            >
              <InputText
                value={form.ca.dir}
                className="w-full"
                onChange={(e) => patch((f) => ({ ...f, ca: { dir: e.target.value } }))}
              />
              <PathResolved dataDir={form.data_dir} value={form.ca.dir} />
            </Field>
            <Field
              label={t("settings.logDb")}
              hint={t("settings.logDbHint")}
              apply="restart"
              error={fieldErrors["logs.db_path"]}
            >
              <InputText
                value={form.logs.db_path}
                className="w-full"
                onChange={(e) =>
                  patch((f) => ({ ...f, logs: { ...f.logs, db_path: e.target.value } }))
                }
              />
              <PathResolved dataDir={form.data_dir} value={form.logs.db_path} />
            </Field>
          </div>

          <p className="text-color-secondary mb-0" style={{ fontSize: "0.85rem" }}>
            {t("settings.pathsHint")}
          </p>
        </Card>

        {/* Actions */}
        <Card>
          <SectionTitle>{t("settings.actions")}</SectionTitle>
          <p style={{ marginTop: 0 }}>{t("settings.actionsDesc")}</p>
          <p className="text-color-secondary" style={{ fontSize: "0.85rem", marginTop: 0 }}>
            {t("settings.applyLegend")}
          </p>
          <div className="flex flex-wrap gap-2">
            <Button
              label={t("common.save")}
              icon="pi pi-save"
              disabled={!dirty}
              loading={saveBusy}
              onClick={handleSave}
              tooltip={dirty ? undefined : t("settings.noChanges")}
            />
            <Button
              label={t("settings.reload")}
              icon="pi pi-refresh"
              severity="secondary"
              loading={busy}
              onClick={handleReload}
            />
            <Button
              label={t("common.cancel")}
              icon="pi pi-undo"
              severity="secondary"
              text
              disabled={!dirty}
              onClick={() => load()}
            />
            <Button
              label={t("settings.restartAccept")}
              icon="pi pi-power-off"
              severity="danger"
              outlined
              loading={restartBusy}
              onClick={handleRestartClick}
            />
          </div>
          {reloadMsg && (
            <Message
              severity={reloadMsg.ok ? "success" : "error"}
              text={reloadMsg.text}
              className="mt-2 w-full"
            />
          )}
          <Message severity="info" text={t("settings.hotReloadDesc")} className="mt-2 w-full" />
        </Card>
      </div>
    </>
  );
}

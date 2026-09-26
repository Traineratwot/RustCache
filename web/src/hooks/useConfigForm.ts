/**
 * Config form state for the Settings page: load, dirty tracking, field patches,
 * save/reload/restart flows, and field-level validation errors.
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { getConfig, reloadConfig, updateConfig } from "../api/client";
import type { Config, ConfigUpdateResult } from "../api/types";

export interface ConfigFormApi {
  form: Config | null;
  original: Config | null;
  dirty: boolean;
  loading: boolean;
  busy: boolean;
  saveBusy: boolean;
  restartBusy: boolean;
  setRestartBusy: (b: boolean) => void;
  fieldErrors: Record<string, string>;
  restartFields: string[];
  reloadMsg: { ok: boolean; text: string } | null;
  /** Apply a patch to the form. Clears the edited field's error when `field` is set. */
  patch: (fn: (f: Config) => Config, field?: string) => void;
  reload: () => Promise<void>;
  save: () => Promise<ConfigUpdateResult | null>;
  setFieldErrors: (e: Record<string, string>) => void;
  setReloadMsg: (m: { ok: boolean; text: string } | null) => void;
  setRestartFields: (f: string[]) => void;
  load: () => Promise<void>;
}

function cloneForm(c: Config): Config {
  return JSON.parse(JSON.stringify(c)) as Config;
}

/**
 * Owns Settings form lifecycle. Pages supply toast text via callbacks so the
 * hook stays free of i18n imports (keeps it unit-testable).
 */
export function useConfigForm(strings: {
  loadError: string;
  reloaded: string;
  reloadError: string;
  reloadApiError: string;
  saved: string;
  savedRestart: string;
  saveError: string;
  saveError2: string;
  restarting: string;
  restarted: string;
  restartError: string;
  onToast: (severity: "success" | "error" | "warn", summary: string, detail: string) => void;
}): ConfigFormApi {
  const [form, setForm] = useState<Config | null>(null);
  const [original, setOriginal] = useState<Config | null>(null);
  const [busy, setBusy] = useState(false);
  const [saveBusy, setSaveBusy] = useState(false);
  const [restartBusy, setRestartBusy] = useState(false);
  const [reloadMsg, setReloadMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const [restartFields, setRestartFields] = useState<string[]>([]);
  const [fieldErrors, setFieldErrors] = useState<Record<string, string>>({});
  const [loading, setLoading] = useState(true);
  const stringsRef = useRef(strings);
  stringsRef.current = strings;

  const dirty = useMemo(
    () => form !== null && original !== null && JSON.stringify(form) !== JSON.stringify(original),
    [form, original],
  );

  const load = useCallback(async () => {
    const s = stringsRef.current;
    setLoading(true);
    try {
      const c = await getConfig();
      setForm(c);
      setOriginal(cloneForm(c));
      setFieldErrors({});
    } catch {
      s.onToast("error", s.loadError, s.loadError);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const patch = useCallback((fn: (f: Config) => Config, field?: string) => {
    setForm((prev) => (prev ? fn(prev) : prev));
    if (field) {
      setFieldErrors((prev) => {
        if (!(field in prev)) return prev;
        const next = { ...prev };
        delete next[field];
        return next;
      });
    }
  }, []);

  const reload = useCallback(async () => {
    const s = stringsRef.current;
    setBusy(true);
    try {
      const r = await reloadConfig();
      if (r.ok) {
        setReloadMsg({ ok: true, text: s.reloaded });
        s.onToast("success", s.reloaded, s.reloaded);
        if (r.config) {
          setForm(r.config);
          setOriginal(cloneForm(r.config));
        } else {
          await load();
        }
      } else {
        setReloadMsg({ ok: false, text: r.error ?? s.reloadError });
      }
    } catch {
      setReloadMsg({ ok: false, text: s.reloadApiError });
    } finally {
      setBusy(false);
    }
  }, [load]);

  const save = useCallback(async (): Promise<ConfigUpdateResult | null> => {
    const s = stringsRef.current;
    if (!form) return null;
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
          s.onToast(
            "warn",
            s.savedRestart,
            s.savedRestart.replace("{fields}", (r.restart_fields ?? []).join(", ")),
          );
        } else {
          s.onToast("success", s.saved, s.saved);
        }
      } else if (r.errors && r.errors.length > 0) {
        const map: Record<string, string> = {};
        for (const e of r.errors) map[e.field] = e.message;
        setFieldErrors(map);
        s.onToast("error", s.saveError, r.error ?? s.saveError);
      } else {
        s.onToast("error", s.saveError, r.error ?? s.saveError);
      }
      return r;
    } catch {
      s.onToast("error", s.saveError, s.saveError2);
      return null;
    } finally {
      setSaveBusy(false);
    }
  }, [form]);

  return {
    form,
    original,
    dirty,
    loading,
    busy,
    saveBusy,
    restartBusy,
    setRestartBusy,
    fieldErrors,
    restartFields,
    reloadMsg,
    patch,
    reload,
    save,
    setFieldErrors,
    setReloadMsg,
    setRestartFields,
    load,
  };
}

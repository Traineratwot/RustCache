import { Button } from "primereact/button";
import { Card } from "primereact/card";
import { ConfirmDialog, confirmDialog } from "primereact/confirmdialog";
import { ProgressBar } from "primereact/progressbar";
import { Toast } from "primereact/toast";
import { useCallback, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { getCache, getConfig, purgeCache } from "../api/client";
import type { CacheInfo, Config } from "../api/types";
import { usePolling } from "../hooks/usePolling";
import { CACHE_POLL_MS } from "../lib/constants";
import { fmtBytes } from "../lib/format";

/** Cache: disk usage vs cap and purge action. */
export default function Cache() {
  const { t } = useTranslation();
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
        summary: t("common.error"),
        detail: t("cache.loadError"),
      });
    }
  }, [t]);

  usePolling(load, CACHE_POLL_MS);

  const maxBytes = cfg?.cache.max_bytes ?? 0;
  const used = info?.bytes ?? 0;
  const pct = maxBytes > 0 ? Math.min(100, Math.round((used / maxBytes) * 100)) : 0;

  const handlePurge = () => {
    confirmDialog({
      message: t("cache.purgeConfirm"),
      header: t("cache.purgeTitle"),
      icon: "pi pi-exclamation-triangle",
      accept: async () => {
        setBusy(true);
        try {
          const r = await purgeCache();
          toast.current?.show({
            severity: "success",
            summary: t("common.done"),
            detail: t("cache.purged", { count: r.purged }),
          });
          await load();
        } catch {
          toast.current?.show({
            severity: "error",
            summary: t("common.error"),
            detail: t("cache.purgeError"),
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
      <h1 className="page-title">{t("cache.title")}</h1>

      <div className="stat-grid">
        <div className="stat-card">
          <div className="label">{t("cache.size")}</div>
          <div className="value accent">{fmtBytes(used)}</div>
        </div>
        <div className="stat-card">
          <div className="label">{t("cache.limit")}</div>
          <div className="value">{maxBytes > 0 ? fmtBytes(maxBytes) : "—"}</div>
        </div>
        <div className="stat-card">
          <div className="label">{t("cache.entries")}</div>
          <div className="value">{info ? String(info.entries) : "—"}</div>
        </div>
        <div className="stat-card">
          <div className="label">{t("cache.used")}</div>
          <div className="value accent">{pct}%</div>
        </div>
      </div>

      <Card title={t("cache.fill")} className="mt-3">
        <ProgressBar value={pct} showValue style={{ height: "14px" }} />
        <p className="mt-2">
          {t("cache.dirLabel")}: <code>{cfg?.cache.dir ?? "—"}</code>
        </p>
      </Card>

      <Card title={t("cache.clear")} className="mt-3">
        <p style={{ marginTop: 0 }}>{t("cache.clearDesc")}</p>
        <Button
          label={t("cache.clearAll")}
          icon="pi pi-trash"
          severity="danger"
          loading={busy}
          onClick={handlePurge}
        />
      </Card>
    </>
  );
}

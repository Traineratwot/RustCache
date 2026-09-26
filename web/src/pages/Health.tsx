import { Badge } from "primereact/badge";
import { Button } from "primereact/button";
import { Card } from "primereact/card";
import { Column } from "primereact/column";
import { DataTable } from "primereact/datatable";
import { Tag } from "primereact/tag";
import { useCallback, useState } from "react";
import { useTranslation } from "react-i18next";
import { getHealth } from "../api/client";
import type { HealthInfo, ListenerInfo } from "../api/types";
import { usePolling } from "../hooks/usePolling";
import { DEFAULT_PORTS, HEALTH_POLL_MS } from "../lib/constants";
import { fmtUptime } from "../lib/format";

function RunningBadge({ running }: { running: boolean }) {
  const { t } = useTranslation();
  return running ? (
    <Tag value={t("health.running")} severity="success" icon="pi pi-check" />
  ) : (
    <Tag value={t("health.notRunning")} severity="danger" icon="pi pi-times" />
  );
}

/** Health: listener bind status and API uptime. */
export default function Health() {
  const { t } = useTranslation();
  const [health, setHealth] = useState<HealthInfo | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const h = await getHealth();
      setHealth(h);
      setError(null);
    } catch {
      setError(t("health.loadError"));
    }
  }, [t]);

  usePolling(load, HEALTH_POLL_MS);

  const listeners = health?.listeners ?? [];
  const allOk = listeners.every((l) => l.running);

  return (
    <>
      <div className="flex align-items-center gap-2 mb-3">
        <h1 className="page-title" style={{ marginBottom: 0 }}>
          {t("health.title")}
        </h1>
        {health && (
          <Badge
            value={allOk ? t("health.allOk") : t("health.hasIssues")}
            severity={allOk ? "success" : "danger"}
            style={{ fontSize: "0.85rem" }}
          />
        )}
        <div className="flex-1" />
        <Button icon="pi pi-refresh" label={t("common.refresh")} text onClick={load} />
      </div>

      {error && <p style={{ color: "var(--red-500)" }}>{error}</p>}

      {health && (
        <div className="stat-grid mb-3">
          <div className="stat-card">
            <div className="label">{t("health.overall")}</div>
            <div className="value accent">
              {health.ok ? t("health.apiOk") : t("health.apiError")}
            </div>
          </div>
          <div className="stat-card">
            <div className="label">{t("health.uptime")}</div>
            <div className="value">{fmtUptime(health.uptime_s)}</div>
          </div>
          <div className="stat-card">
            <div className="label">{t("health.listenersCount")}</div>
            <div className="value">
              {listeners.filter((l) => l.running).length} / {listeners.length}
            </div>
          </div>
          <div className="stat-card">
            <div className="label">{t("health.autoRefresh")}</div>
            <div className="value" style={{ fontSize: "1.1rem" }}>
              {t("health.autoRefreshValue")}
            </div>
          </div>
        </div>
      )}

      <Card title={t("health.listeners")}>
        <DataTable value={listeners} emptyMessage={t("common.empty")} size="small" stripedRows>
          <Column
            field="name"
            header={t("health.service")}
            body={(l: ListenerInfo) => <span className="font-medium">{l.name}</span>}
          />
          <Column
            field="bind"
            header={t("health.bind")}
            body={(l: ListenerInfo) => <code>{l.bind}</code>}
          />
          <Column
            field="port"
            header={t("health.port")}
            body={(l: ListenerInfo) => (
              <code style={{ fontSize: "1rem", fontWeight: 600 }}>{l.port}</code>
            )}
            style={{ width: "100px" }}
          />
          <Column
            field="running"
            header={t("health.status")}
            body={(l: ListenerInfo) => <RunningBadge running={l.running} />}
            style={{ width: "160px" }}
          />
        </DataTable>
      </Card>

      <Card title={t("health.defaultPorts")} className="mt-3">
        <p style={{ marginTop: 0 }}>
          <Tag value={String(DEFAULT_PORTS.http)} severity="info" /> {t("health.portHttp")}
          <br />
          <Tag value={String(DEFAULT_PORTS.https)} severity="info" /> {t("health.portHttps")}
          <br />
          <Tag value={String(DEFAULT_PORTS.socks5)} severity="secondary" /> {t("health.portSocks")}
          <br />
          <Tag value={String(DEFAULT_PORTS.api)} severity="success" /> {t("health.portApi")}
        </p>
      </Card>
    </>
  );
}

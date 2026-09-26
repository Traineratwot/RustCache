import { Button } from "primereact/button";
import { Column } from "primereact/column";
import { ConfirmDialog, confirmDialog } from "primereact/confirmdialog";
import { DataTable } from "primereact/datatable";
import { Dropdown } from "primereact/dropdown";
import { InputText } from "primereact/inputtext";
import { Tag } from "primereact/tag";
import { Toast } from "primereact/toast";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { clearRequests, getRequests } from "../api/client";
import type { ReqRecord, RequestQuery } from "../api/types";
import { useDebounced } from "../hooks/useDebounced";
import { usePolling } from "../hooks/usePolling";
import { REQUESTS_POLL_MS } from "../lib/constants";
import { fmtBytes, fmtDateTime } from "../lib/format";
import { outcomeSeverity } from "../lib/outcome";

function statusRange(code: string): { status_min?: number; status_max?: number } {
  if (!code) return {};
  const n = Number.parseInt(code, 10);
  return { status_min: n * 100, status_max: n * 100 + 99 };
}

function timeSince(key: string): number | undefined {
  if (!key) return undefined;
  const now = Date.now();
  switch (key) {
    case "1h":
      return now - 3600_000;
    case "24h":
      return now - 86_400_000;
    case "7d":
      return now - 7 * 86_400_000;
    case "30d":
      return now - 30 * 86_400_000;
    default:
      return undefined;
  }
}

/** Requests: filterable, paginated traffic history with auto-refresh. */
export default function Requests() {
  const { t } = useTranslation();
  const [rows, setRows] = useState<ReqRecord[]>([]);
  const [total, setTotal] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const [q, setQ] = useState("");
  const [method, setMethod] = useState("");
  const [outcome, setOutcome] = useState("");
  const [statusCls, setStatusCls] = useState("");
  const [timeRange, setTimeRange] = useState("");
  const [limit, setLimit] = useState(50);
  const [offset, setOffset] = useState(0);
  const [autoRefresh, setAutoRefresh] = useState(true);
  const [reloadTick, setReloadTick] = useState(0);

  const toast = useRef<Toast>(null);
  const reqSeq = useRef(0);

  const methodOptions = useMemo(
    () => [
      { label: t("requests.allMethods"), value: "" },
      { label: "GET", value: "GET" },
      { label: "POST", value: "POST" },
      { label: "PUT", value: "PUT" },
      { label: "DELETE", value: "DELETE" },
      { label: "HEAD", value: "HEAD" },
      { label: "OPTIONS", value: "OPTIONS" },
      { label: "CONNECT", value: "CONNECT" },
      { label: "PATCH", value: "PATCH" },
    ],
    [t],
  );

  const outcomeOptions = useMemo(
    () => [
      { label: t("requests.allOutcomes"), value: "" },
      { label: "HIT", value: "HIT" },
      { label: "MISS", value: "MISS" },
      { label: "HIT_REVALIDATED", value: "HIT_REVALIDATED" },
      { label: "HIT_STALE", value: "HIT_STALE" },
      { label: "REVALIDATED", value: "REVALIDATED" },
      { label: "BYPASS", value: "BYPASS" },
      { label: "TUNNEL", value: "TUNNEL" },
      { label: "REJECT_CMD", value: "REJECT_CMD" },
      { label: "ERROR", value: "ERROR" },
    ],
    [t],
  );

  const statusOptions = useMemo(
    () => [
      { label: t("requests.allStatuses"), value: "" },
      { label: "2xx", value: "2xx" },
      { label: "3xx", value: "3xx" },
      { label: "4xx", value: "4xx" },
      { label: "5xx", value: "5xx" },
    ],
    [t],
  );

  const timeOptions = useMemo(
    () => [
      { label: t("requests.allTime"), value: "" },
      { label: t("requests.time1h"), value: "1h" },
      { label: t("requests.time24h"), value: "24h" },
      { label: t("requests.time7d"), value: "7d" },
      { label: t("requests.time30d"), value: "30d" },
    ],
    [t],
  );

  // Debounce free-text search so typing does not fire one request per keystroke.
  const debouncedQ = useDebounced(q, 300);

  const load = useCallback(async () => {
    const seq = ++reqSeq.current;
    const query: RequestQuery = {
      q: debouncedQ.trim() || undefined,
      method: method || undefined,
      outcome: outcome || undefined,
      ...statusRange(statusCls),
      since: timeSince(timeRange),
      limit,
      offset,
    };
    try {
      const r = await getRequests(query);
      if (seq !== reqSeq.current) return;
      setRows(r.requests ?? []);
      setTotal(r.total ?? r.requests?.length ?? 0);
      setError(null);
    } catch {
      if (seq !== reqSeq.current) return;
      setError(t("requests.loadError"));
    }
  }, [debouncedQ, method, outcome, statusCls, timeRange, limit, offset, t]);

  useEffect(() => {
    void reloadTick;
    load();
  }, [load, reloadTick]);

  usePolling(load, REQUESTS_POLL_MS, autoRefresh);

  const onFilterChange = (setter: (v: string) => void) => (value: unknown) => {
    setter(String(value ?? ""));
    setOffset(0);
    setReloadTick((n) => n + 1);
  };

  const handleClear = () => {
    confirmDialog({
      message: t("requests.clearConfirm"),
      header: t("requests.clearTitle"),
      icon: "pi pi-exclamation-triangle",
      acceptClassName: "p-button-danger",
      accept: async () => {
        setBusy(true);
        try {
          const r = await clearRequests();
          toast.current?.show({
            severity: "success",
            summary: t("common.done"),
            detail: t("requests.deleted", { count: r.deleted }),
          });
          setOffset(0);
          await load();
        } catch {
          toast.current?.show({
            severity: "error",
            summary: t("common.error"),
            detail: t("requests.clearError"),
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
      <h1 className="page-title">{t("requests.title")}</h1>
      {error && <p style={{ color: "var(--red-500)" }}>{error}</p>}

      <div className="flex flex-wrap align-items-center gap-2 mb-3">
        <span className="p-input-icon-left">
          <i className="pi pi-search" />
          <InputText
            value={q}
            onChange={(e) => {
              setQ(e.target.value);
              setOffset(0);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") setReloadTick((n) => n + 1);
            }}
            placeholder={t("requests.searchPlaceholder")}
            style={{ width: "240px" }}
          />
        </span>
        <Dropdown
          value={method}
          options={methodOptions}
          optionLabel="label"
          optionValue="value"
          onChange={(e) => onFilterChange(setMethod)(e.value)}
          style={{ width: "140px" }}
        />
        <Dropdown
          value={outcome}
          options={outcomeOptions}
          optionLabel="label"
          optionValue="value"
          onChange={(e) => onFilterChange(setOutcome)(e.value)}
          style={{ width: "170px" }}
        />
        <Dropdown
          value={statusCls}
          options={statusOptions}
          optionLabel="label"
          optionValue="value"
          onChange={(e) => onFilterChange(setStatusCls)(e.value)}
          style={{ width: "130px" }}
        />
        <Dropdown
          value={timeRange}
          options={timeOptions}
          optionLabel="label"
          optionValue="value"
          onChange={(e) => onFilterChange(setTimeRange)(e.value)}
          style={{ width: "130px" }}
        />
        <Button
          label={autoRefresh ? t("requests.pause") : t("requests.resume")}
          icon={autoRefresh ? "pi pi-pause" : "pi pi-play"}
          text
          onClick={() => setAutoRefresh((v) => !v)}
        />
        <Button
          label={t("requests.clear")}
          icon="pi pi-trash"
          severity="danger"
          outlined
          loading={busy}
          onClick={handleClear}
        />
        <span className="text-color-secondary">
          {autoRefresh ? t("requests.autoRefreshOn") : t("requests.autoRefreshOff")}
          {t("requests.records", { count: total })}
        </span>
      </div>

      <DataTable
        value={rows}
        size="small"
        stripedRows
        emptyMessage={t("requests.empty")}
        responsiveLayout="scroll"
        paginator
        rows={limit}
        totalRecords={total}
        first={offset}
        rowsPerPageOptions={[20, 50, 100, 200]}
        onPage={(e) => {
          setLimit(e.rows);
          setOffset(e.first);
        }}
      >
        <Column
          field="ts"
          header={t("requests.time")}
          body={(r: ReqRecord) => fmtDateTime(r.ts)}
          style={{ width: "140px" }}
        />
        <Column field="method" header={t("requests.method")} style={{ width: "80px" }} />
        <Column field="host" header={t("requests.host")} style={{ width: "180px" }} />
        <Column
          field="url"
          header={t("requests.url")}
          body={(r: ReqRecord) => (
            <span
              title={r.url}
              style={{
                display: "inline-block",
                maxWidth: "320px",
                overflow: "hidden",
                textOverflow: "ellipsis",
                whiteSpace: "nowrap",
                verticalAlign: "bottom",
              }}
            >
              {r.url}
            </span>
          )}
        />
        <Column field="status" header={t("requests.status")} style={{ width: "80px" }} />
        <Column
          field="outcome"
          header={t("requests.result")}
          body={(r: ReqRecord) => <Tag value={r.outcome} severity={outcomeSeverity(r.outcome)} />}
          style={{ width: "130px" }}
        />
        <Column
          field="duration_ms"
          header={t("requests.ms")}
          body={(r: ReqRecord) => `${r.duration_ms}`}
          style={{ width: "70px" }}
        />
        <Column
          field="resp_bytes"
          header={t("requests.size")}
          body={(r: ReqRecord) => fmtBytes(r.resp_bytes)}
          style={{ width: "100px" }}
        />
      </DataTable>
    </>
  );
}

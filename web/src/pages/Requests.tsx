import { Button } from "primereact/button";
import { Column } from "primereact/column";
import { ConfirmDialog, confirmDialog } from "primereact/confirmdialog";
import { DataTable } from "primereact/datatable";
import { Dropdown } from "primereact/dropdown";
import { InputText } from "primereact/inputtext";
import { Tag } from "primereact/tag";
import { Toast } from "primereact/toast";
import { useCallback, useEffect, useRef, useState } from "react";
import { clearRequests, getRequests } from "../api/client";
import type { RequestQuery, ReqRecord } from "../api/types";

function outcomeSeverity(outcome: string): "success" | "info" | "warning" | "danger" | "secondary" {
  switch (outcome) {
    case "HIT":
    case "HIT_REVALIDATED":
      return "success";
    case "MISS":
    case "REVALIDATED":
      return "info";
    case "BYPASS":
    case "REJECT_CMD":
      return "warning";
    case "TUNNEL":
      return "secondary";
    default:
      return "danger";
  }
}

function fmtTs(ts: number): string {
  return new Date(ts).toLocaleTimeString("ru-RU");
}

function fmtBytes(n: number): string {
  if (n < 1024) return `${n} Б`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} КБ`;
  return `${(n / 1024 / 1024).toFixed(2)} МБ`;
}

const METHOD_OPTIONS = [
  { label: "Все методы", value: "" },
  { label: "GET", value: "GET" },
  { label: "POST", value: "POST" },
  { label: "PUT", value: "PUT" },
  { label: "DELETE", value: "DELETE" },
  { label: "HEAD", value: "HEAD" },
  { label: "OPTIONS", value: "OPTIONS" },
  { label: "CONNECT", value: "CONNECT" },
  { label: "PATCH", value: "PATCH" },
];

const OUTCOME_OPTIONS = [
  { label: "Все результаты", value: "" },
  { label: "HIT", value: "HIT" },
  { label: "MISS", value: "MISS" },
  { label: "HIT_REVALIDATED", value: "HIT_REVALIDATED" },
  { label: "REVALIDATED", value: "REVALIDATED" },
  { label: "BYPASS", value: "BYPASS" },
  { label: "TUNNEL", value: "TUNNEL" },
  { label: "REJECT_CMD", value: "REJECT_CMD" },
  { label: "ERROR", value: "ERROR" },
];

const STATUS_OPTIONS = [
  { label: "Все статусы", value: "" },
  { label: "2xx", value: "2xx" },
  { label: "3xx", value: "3xx" },
  { label: "4xx", value: "4xx" },
  { label: "5xx", value: "5xx" },
];

const TIME_OPTIONS = [
  { label: "Всё время", value: "" },
  { label: "1 час", value: "1h" },
  { label: "24 часа", value: "24h" },
  { label: "7 дней", value: "7d" },
  { label: "30 дней", value: "30d" },
];

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

export default function Requests() {
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

  const load = useCallback(async () => {
    const seq = ++reqSeq.current;
    const query: RequestQuery = {
      q: q.trim() || undefined,
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
      setError("Не удалось загрузить запросы");
    }
  }, [q, method, outcome, statusCls, timeRange, limit, offset]);

  useEffect(() => {
    load();
  }, [load, reloadTick]);

  useEffect(() => {
    if (!autoRefresh) return;
    const id = setInterval(() => {
      void load();
    }, 2000);
    return () => clearInterval(id);
  }, [autoRefresh, load]);

  const onFilterChange = (setter: (v: string) => void) => (value: unknown) => {
    setter(String(value ?? ""));
    setOffset(0);
    setReloadTick((t) => t + 1);
  };

  const handleClear = () => {
    confirmDialog({
      message: "Удалить все записи журнала запросов?",
      header: "Очистка журнала",
      icon: "pi pi-exclamation-triangle",
      acceptClassName: "p-button-danger",
      accept: async () => {
        setBusy(true);
        try {
          const r = await clearRequests();
          toast.current?.show({
            severity: "success",
            summary: "Готово",
            detail: `Удалено записей: ${r.deleted}`,
          });
          setOffset(0);
          await load();
        } catch {
          toast.current?.show({
            severity: "error",
            summary: "Ошибка",
            detail: "Не удалось очистить журнал",
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
      <h1 className="page-title">Запросы</h1>
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
              if (e.key === "Enter") setReloadTick((t) => t + 1);
            }}
            placeholder="Поиск по URL, host..."
            style={{ width: "240px" }}
          />
        </span>
        <Dropdown
          value={method}
          options={METHOD_OPTIONS}
          optionLabel="label"
          optionValue="value"
          onChange={(e) => onFilterChange(setMethod)(e.value)}
          style={{ width: "140px" }}
        />
        <Dropdown
          value={outcome}
          options={OUTCOME_OPTIONS}
          optionLabel="label"
          optionValue="value"
          onChange={(e) => onFilterChange(setOutcome)(e.value)}
          style={{ width: "170px" }}
        />
        <Dropdown
          value={statusCls}
          options={STATUS_OPTIONS}
          optionLabel="label"
          optionValue="value"
          onChange={(e) => onFilterChange(setStatusCls)(e.value)}
          style={{ width: "130px" }}
        />
        <Dropdown
          value={timeRange}
          options={TIME_OPTIONS}
          optionLabel="label"
          optionValue="value"
          onChange={(e) => onFilterChange(setTimeRange)(e.value)}
          style={{ width: "130px" }}
        />
        <Button
          label={autoRefresh ? "Пауза" : "Продолжить"}
          icon={autoRefresh ? "pi pi-pause" : "pi pi-play"}
          text
          onClick={() => setAutoRefresh((v) => !v)}
        />
        <Button
          label="Очистить"
          icon="pi pi-trash"
          severity="danger"
          outlined
          loading={busy}
          onClick={handleClear}
        />
        <span className="text-color-secondary">
          {autoRefresh ? "Автообновление 2 с · " : "Пауза · "}
          {total} записей
        </span>
      </div>

      <DataTable
        value={rows}
        size="small"
        stripedRows
        emptyMessage="Нет запросов"
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
          header="Время"
          body={(r: ReqRecord) => fmtTs(r.ts)}
          style={{ width: "100px" }}
        />
        <Column field="method" header="Метод" style={{ width: "80px" }} />
        <Column field="host" header="Host" style={{ width: "180px" }} />
        <Column
          field="url"
          header="URL"
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
        <Column field="status" header="Статус" style={{ width: "80px" }} />
        <Column
          field="outcome"
          header="Результат"
          body={(r: ReqRecord) => <Tag value={r.outcome} severity={outcomeSeverity(r.outcome)} />}
          style={{ width: "130px" }}
        />
        <Column
          field="duration_ms"
          header="Мс"
          body={(r: ReqRecord) => `${r.duration_ms}`}
          style={{ width: "70px" }}
        />
        <Column
          field="resp_bytes"
          header="Размер"
          body={(r: ReqRecord) => fmtBytes(r.resp_bytes)}
          style={{ width: "100px" }}
        />
      </DataTable>
    </>
  );
}

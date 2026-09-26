import { Button } from "primereact/button";
import { Card } from "primereact/card";
import { Chart } from "primereact/chart";
import { Column } from "primereact/column";
import { DataTable } from "primereact/datatable";
import { ProgressBar } from "primereact/progressbar";
import { Tag } from "primereact/tag";
import { useCallback, useEffect, useState } from "react";
import { getLogStats } from "../api/client";
import type { HostStat, LogStats, OutcomeStat, SeriesPoint } from "../api/types";

function fmtBytes(n: number): string {
  if (n < 1024) return `${n} Б`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} КБ`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} МБ`;
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} ГБ`;
}

function fmtBucket(ms: number): string {
  if (ms < 60_000) return `${Math.round(ms / 1000)} с`;
  if (ms < 3_600_000) return `${Math.round(ms / 60_000)} мин`;
  if (ms < 86_400_000) return `${Math.round(ms / 3_600_000)} ч`;
  return `${Math.round(ms / 86_400_000)} д`;
}

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

function StatCard({ label, value, accent }: { label: string; value: string; accent?: boolean }) {
  return (
    <div className="stat-card">
      <div className="label">{label}</div>
      <div className={accent ? "value accent" : "value"}>{value}</div>
    </div>
  );
}

function seriesChart(series: SeriesPoint[], bucketMs: number) {
  const labels = series.map((p) => {
    const d = new Date(p.ts);
    return bucketMs >= 86_400_000
      ? d.toLocaleDateString("ru-RU")
      : d.toLocaleTimeString("ru-RU", { hour: "2-digit", minute: "2-digit" });
  });
  return {
    labels,
    datasets: [
      {
        label: "Попадания",
        data: series.map((p) => p.hits),
        backgroundColor: "rgba(46, 160, 67, 0.85)",
        stack: "s",
      },
      {
        label: "Промахи",
        data: series.map((p) => p.miss_like),
        backgroundColor: "rgba(56, 132, 255, 0.8)",
        stack: "s",
      },
      {
        label: "Прочее",
        data: series.map((p) => Math.max(0, p.count - p.hits - p.miss_like)),
        backgroundColor: "rgba(160, 160, 170, 0.7)",
        stack: "s",
      },
    ],
  };
}

const CHART_OPTIONS = {
  responsive: true,
  maintainAspectRatio: false,
  plugins: {
    legend: { position: "bottom" as const },
  },
  scales: {
    x: { stacked: true, ticks: { maxTicksLimit: 12 } },
    y: { stacked: true, beginAtZero: true },
  },
};

export default function Dashboard() {
  const [stats, setStats] = useState<LogStats | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const s = await getLogStats();
      setStats(s);
      setError(null);
    } catch {
      setError("Не удалось получить статистику журнала");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const header = (
    <div className="page-header">
      <h1 className="page-title">Обзор</h1>
      <Button
        label="Обновить"
        icon="pi pi-refresh"
        onClick={load}
        loading={loading}
        outlined
        size="small"
      />
    </div>
  );

  if (error) {
    return (
      <>
        {header}
        <Card>
          <p>{error}</p>
          <p>Убедитесь, что rustcache запущен и API доступен на 127.0.0.1:8080.</p>
        </Card>
      </>
    );
  }

  if (!stats) {
    return (
      <>
        {header}
        <p>Загрузка...</p>
      </>
    );
  }

  const hitPct = Math.round(stats.hit_rate * 100);
  const hasSeries = stats.series.length > 0;

  return (
    <>
      {header}
      <div className="stat-grid">
        <StatCard label="Hit rate" value={`${hitPct}%`} accent />
        <StatCard label="Всего запросов" value={String(stats.total)} />
        <StatCard label="Попадания (HIT)" value={String(stats.hits)} />
        <StatCard label="Промахи (MISS/REVALIDATED)" value={String(stats.miss_like)} />
        <StatCard label="Средняя длительность" value={`${stats.avg_duration_ms.toFixed(1)} мс`} />
        <StatCard label="Макс. длительность" value={`${stats.max_duration_ms} мс`} />
        <StatCard label="Сэкономлено" value={fmtBytes(stats.bytes_saved)} accent />
        <StatCard label="Отдано" value={fmtBytes(stats.bytes_served)} />
      </div>

      <Card title="Доля попаданий" className="mt-3">
        <ProgressBar value={hitPct} showValue={false} style={{ height: "12px" }} />
        <p className="mt-2">
          HIT {stats.hits} / MISS-REVALIDATED {stats.miss_like} &nbsp;·&nbsp; всего в журнале:{" "}
          {stats.total}
        </p>
      </Card>

      <Card title="Запросы по времени" className="mt-3">
        {hasSeries ? (
          <>
            <div className="chart-wrap">
              <Chart
                type="bar"
                data={seriesChart(stats.series, stats.bucket_ms)}
                options={CHART_OPTIONS}
              />
            </div>
            <p className="chart-caption">Шаг: {fmtBucket(stats.bucket_ms)}</p>
          </>
        ) : (
          <p>Нет данных за период</p>
        )}
      </Card>

      <div className="two-col mt-3">
        <Card title="Исходы">
          <table className="outcome-table">
            <thead>
              <tr>
                <th>Исход</th>
                <th>Запросов</th>
                <th>Байты</th>
                <th>Ср. время</th>
              </tr>
            </thead>
            <tbody>
              {stats.by_outcome.map((o: OutcomeStat) => (
                <tr key={o.outcome}>
                  <td>
                    <Tag value={o.outcome} severity={outcomeSeverity(o.outcome)} />
                  </td>
                  <td>{o.count}</td>
                  <td>{fmtBytes(o.bytes)}</td>
                  <td>{o.avg_duration_ms.toFixed(1)} мс</td>
                </tr>
              ))}
            </tbody>
          </table>
        </Card>

        <Card title="Топ хостов">
          <DataTable value={stats.top_hosts} emptyMessage="Нет данных" size="small" stripedRows>
            <Column field="host" header="Хост" />
            <Column field="count" header="Запросов" />
            <Column field="bytes" header="Байты" body={(r: HostStat) => fmtBytes(r.bytes)} />
            <Column
              field="hit_rate"
              header="Hit rate"
              body={(r: HostStat) => `${Math.round(r.hit_rate * 100)}%`}
            />
          </DataTable>
        </Card>
      </div>
    </>
  );
}

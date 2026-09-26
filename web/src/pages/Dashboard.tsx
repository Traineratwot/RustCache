import { Button } from "primereact/button";
import { Card } from "primereact/card";
import { Chart } from "primereact/chart";
import { Column } from "primereact/column";
import { DataTable } from "primereact/datatable";
import { ProgressBar } from "primereact/progressbar";
import { Tag } from "primereact/tag";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { getLogStats } from "../api/client";
import type { HostStat, LogStats, OutcomeStat, SeriesPoint } from "../api/types";
import { fmtBucket, fmtBytes, fmtDate, fmtMs, fmtTime } from "../lib/format";
import { usePrefs } from "../prefs/PrefsContext";

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

function seriesChart(
  series: SeriesPoint[],
  bucketMs: number,
  labels: { hits: string; misses: string; other: string },
) {
  const axis = series.map((p) => {
    const d = new Date(p.ts);
    return bucketMs >= 86_400_000 ? fmtDate(d) : fmtTime(d);
  });
  return {
    labels: axis,
    datasets: [
      {
        label: labels.hits,
        data: series.map((p) => p.hits),
        backgroundColor: "rgba(46, 160, 67, 0.85)",
        stack: "s",
      },
      {
        label: labels.misses,
        data: series.map((p) => p.miss_like),
        backgroundColor: "rgba(56, 132, 255, 0.8)",
        stack: "s",
      },
      {
        label: labels.other,
        data: series.map((p) => Math.max(0, p.count - p.hits - p.miss_like)),
        backgroundColor: "rgba(160, 160, 170, 0.7)",
        stack: "s",
      },
    ],
  };
}

function useChartTheme() {
  const { resolvedTheme } = usePrefs();
  return useMemo(() => {
    const cs = getComputedStyle(document.documentElement);
    const fallback =
      resolvedTheme === "dark"
        ? { color: "#9ca3af", border: "#374151" }
        : { color: "#6b7280", border: "#dfe7ef" };
    return {
      color: cs.getPropertyValue("--text-color-secondary").trim() || fallback.color,
      border: cs.getPropertyValue("--surface-border").trim() || fallback.border,
    };
  }, [resolvedTheme]);
}

export default function Dashboard() {
  const { t } = useTranslation();
  const chartTheme = useChartTheme();
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
      setError(t("dashboard.loadError"));
    } finally {
      setLoading(false);
    }
  }, [t]);

  useEffect(() => {
    load();
  }, [load]);

  const chartOptions = {
    responsive: true,
    maintainAspectRatio: false,
    plugins: {
      legend: { position: "bottom" as const, labels: { color: chartTheme.color } },
    },
    scales: {
      x: {
        stacked: true,
        ticks: { maxTicksLimit: 12, color: chartTheme.color },
        grid: { color: chartTheme.border },
      },
      y: {
        stacked: true,
        beginAtZero: true,
        ticks: { color: chartTheme.color },
        grid: { color: chartTheme.border },
      },
    },
  };

  const header = (
    <div className="page-header">
      <h1 className="page-title">{t("dashboard.title")}</h1>
      <Button
        label={t("common.refresh")}
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
          <p>{t("dashboard.apiHint")}</p>
        </Card>
      </>
    );
  }

  if (!stats) {
    return (
      <>
        {header}
        <p>{t("common.loading")}</p>
      </>
    );
  }

  const hitPct = Math.round(stats.hit_rate * 100);
  const hasSeries = stats.series.length > 0;

  return (
    <>
      {header}
      <div className="stat-grid">
        <StatCard label={t("dashboard.hitRate")} value={`${hitPct}%`} accent />
        <StatCard label={t("dashboard.total")} value={String(stats.total)} />
        <StatCard label={t("dashboard.hits")} value={String(stats.hits)} />
        <StatCard label={t("dashboard.missLike")} value={String(stats.miss_like)} />
        <StatCard label={t("dashboard.avgDuration")} value={fmtMs(stats.avg_duration_ms)} />
        <StatCard label={t("dashboard.maxDuration")} value={fmtMs(stats.max_duration_ms, 0)} />
        <StatCard label={t("dashboard.saved")} value={fmtBytes(stats.bytes_saved)} accent />
        <StatCard label={t("dashboard.served")} value={fmtBytes(stats.bytes_served)} />
      </div>

      <Card title={t("dashboard.hitRate")} className="mt-3">
        <ProgressBar value={hitPct} showValue={false} style={{ height: "12px" }} />
        <p className="mt-2">
          {t("dashboard.hitMissLine", {
            hits: stats.hits,
            miss: stats.miss_like,
            total: stats.total,
          })}
        </p>
      </Card>

      <Card title={t("dashboard.overTime")} className="mt-3">
        {hasSeries ? (
          <>
            <div className="chart-wrap">
              <Chart
                type="bar"
                data={seriesChart(stats.series, stats.bucket_ms, {
                  hits: t("dashboard.chartHits"),
                  misses: t("dashboard.chartMisses"),
                  other: t("dashboard.chartOther"),
                })}
                options={chartOptions}
              />
            </div>
            <p className="chart-caption">
              {t("dashboard.bucketLabel", { value: fmtBucket(stats.bucket_ms) })}
            </p>
          </>
        ) : (
          <p>{t("dashboard.noDataPeriod")}</p>
        )}
      </Card>

      <div className="two-col mt-3">
        <Card title={t("dashboard.outcomes")}>
          <table className="outcome-table">
            <thead>
              <tr>
                <th>{t("dashboard.outcome")}</th>
                <th>{t("dashboard.count")}</th>
                <th>{t("dashboard.bytes")}</th>
                <th>{t("dashboard.avgTime")}</th>
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
                  <td>{fmtMs(o.avg_duration_ms)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </Card>

        <Card title={t("dashboard.topHosts")}>
          <DataTable
            value={stats.top_hosts}
            emptyMessage={t("common.empty")}
            size="small"
            stripedRows
          >
            <Column field="host" header={t("dashboard.host")} />
            <Column field="count" header={t("dashboard.count")} />
            <Column
              field="bytes"
              header={t("dashboard.bytes")}
              body={(r: HostStat) => fmtBytes(r.bytes)}
            />
            <Column
              field="hit_rate"
              header={t("dashboard.hitRate")}
              body={(r: HostStat) => `${Math.round(r.hit_rate * 100)}%`}
            />
          </DataTable>
        </Card>
      </div>
    </>
  );
}

import { Card } from "primereact/card";
import { ProgressBar } from "primereact/progressbar";
import { useEffect, useState } from "react";
import { getStats } from "../api/client";
import type { Stats } from "../api/types";

function fmtBytes(n: number): string {
  if (n < 1024) return `${n} Б`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} КБ`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} МБ`;
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} ГБ`;
}

function StatCard({ label, value, accent }: { label: string; value: string; accent?: boolean }) {
  return (
    <div className="stat-card">
      <div className="label">{label}</div>
      <div className={accent ? "value accent" : "value"}>{value}</div>
    </div>
  );
}

export default function Dashboard() {
  const [stats, setStats] = useState<Stats | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    const load = async () => {
      try {
        const s = await getStats();
        if (alive) {
          setStats(s);
          setError(null);
        }
      } catch {
        if (alive) setError("Не удалось получить статистику API");
      }
    };
    load();
    const id = setInterval(load, 2000);
    return () => {
      alive = false;
      clearInterval(id);
    };
  }, []);

  if (error) {
    return (
      <>
        <h1 className="page-title">Обзор</h1>
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
        <h1 className="page-title">Обзор</h1>
        <p>Загрузка...</p>
      </>
    );
  }

  const hitPct = Math.round(stats.hit_rate * 100);

  return (
    <>
      <h1 className="page-title">Обзор</h1>
      <div className="stat-grid">
        <StatCard label="Hit rate" value={`${hitPct}%`} accent />
        <StatCard label="Попадания (HIT)" value={String(stats.hits)} />
        <StatCard label="Промахи (MISS)" value={String(stats.misses)} />
        <StatCard label="Сэкономлено" value={`${stats.saved_mb.toFixed(2)} МБ`} accent />
        <StatCard label="Отдано" value={fmtBytes(stats.bytes_served)} />
        <StatCard label="Туннели (CONNECT)" value={String(stats.tunnels)} />
        <StatCard label="Обходы (BYPASS)" value={String(stats.bypasses)} />
        <StatCard label="Ревалидации" value={String(stats.revalidations)} />
        <StatCard label="Ошибки" value={String(stats.errors)} />
      </div>

      <Card title="Доля попаданий" className="mt-3">
        <ProgressBar value={hitPct} showValue={false} style={{ height: "12px" }} />
        <p className="mt-2">
          HIT {stats.hits} / MISS {stats.misses} &nbsp;·&nbsp; всего запросов в статистике:{" "}
          {stats.hits + stats.misses}
        </p>
      </Card>
    </>
  );
}

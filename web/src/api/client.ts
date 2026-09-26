import type {
  CacheInfo,
  Config,
  ConfigUpdateResult,
  HealthInfo,
  LogSettings,
  LogStats,
  Matcher,
  NetInfo,
  PacInfo,
  RequestPage,
  RequestQuery,
} from "./types";

async function getJson<T>(url: string): Promise<T> {
  const r = await fetch(url);
  if (!r.ok) throw new Error(`${url} ${r.status}`);
  return r.json();
}

async function sendJson<T>(url: string, method: string, body?: unknown): Promise<T> {
  const r = await fetch(url, {
    method,
    headers: body !== undefined ? { "Content-Type": "application/json" } : undefined,
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
  if (!r.ok) throw new Error(`${url} ${r.status}`);
  return r.json();
}

export function getLogStats(query: { since?: number; until?: number } = {}): Promise<LogStats> {
  const p = new URLSearchParams();
  if (query.since !== undefined) p.set("since", String(query.since));
  if (query.until !== undefined) p.set("until", String(query.until));
  const qs = p.toString();
  return getJson(qs ? `/api/logs/stats?${qs}` : "/api/logs/stats");
}

export function getHealth(): Promise<HealthInfo> {
  return getJson("/api/health");
}

export function getRequests(query: RequestQuery = {}): Promise<RequestPage> {
  const p = new URLSearchParams();
  if (query.q) p.set("q", query.q);
  if (query.method) p.set("method", query.method);
  if (query.outcome) p.set("outcome", query.outcome);
  if (query.status_min !== undefined) p.set("status_min", String(query.status_min));
  if (query.status_max !== undefined) p.set("status_max", String(query.status_max));
  if (query.since !== undefined) p.set("since", String(query.since));
  if (query.until !== undefined) p.set("until", String(query.until));
  p.set("limit", String(query.limit ?? 50));
  p.set("offset", String(query.offset ?? 0));
  return getJson(`/api/requests?${p.toString()}`);
}

export function clearRequests(): Promise<{ ok: boolean; deleted: number }> {
  return sendJson("/api/requests", "DELETE");
}

export function getLogSettings(): Promise<LogSettings> {
  return getJson("/api/logs/settings");
}

export function updateLogSettings(
  body: Partial<LogSettings>,
): Promise<{ ok: boolean; settings: LogSettings }> {
  return sendJson("/api/logs/settings", "PUT", body);
}

export function getExclusions(): Promise<{ exclusions: Matcher[] }> {
  return getJson("/api/exclusions");
}

export function addExclusion(body: { domain?: string; cidr?: string }): Promise<{ ok: boolean }> {
  return sendJson("/api/exclusions", "POST", body);
}

export function deleteExclusion(body: {
  domain?: string;
  cidr?: string;
  all?: boolean;
}): Promise<{ ok: boolean }> {
  return sendJson("/api/exclusions", "DELETE", body);
}

export function getCache(): Promise<CacheInfo> {
  return getJson("/api/cache");
}

export function purgeCache(): Promise<{ ok: boolean; purged: number }> {
  return sendJson("/api/cache", "DELETE");
}

export function getConfig(): Promise<Config> {
  return getJson("/api/config");
}

/**
 * Save the full TOML config. Returns the stored config + restart-required fields.
 * Validation failures come back as `{ok: false, error, errors[]}` (not thrown)
 * so the form can highlight the offending fields.
 */
export async function updateConfig(body: Config): Promise<ConfigUpdateResult> {
  const r = await fetch("/api/config", {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  return (await r.json()) as ConfigUpdateResult;
}

export function getNetInfo(): Promise<NetInfo> {
  return getJson("/api/netinfo");
}

export function getPac(): Promise<PacInfo> {
  return getJson("/api/pac");
}

export function reloadConfig(): Promise<{ ok: boolean; config?: Config; error?: string }> {
  return sendJson("/api/config/reload", "POST");
}

/** Re-exec the process (same argv). `dryRun` only acknowledges — used in tests. */
export function restartProcess(
  dryRun = false,
): Promise<{ ok: boolean; dry_run?: boolean; message?: string; error?: string }> {
  return sendJson(dryRun ? "/api/config/restart?dry_run=1" : "/api/config/restart", "POST");
}

export async function downloadCa(): Promise<string> {
  const r = await fetch("/api/ca.crt");
  if (!r.ok) throw new Error(`ca.crt ${r.status}`);
  return r.text();
}

export async function caFingerprint(pem: string): Promise<string> {
  const b64 = pem
    .replace(/-----BEGIN CERTIFICATE-----/g, "")
    .replace(/-----END CERTIFICATE-----/g, "")
    .replace(/\s+/g, "");
  const bin = atob(b64);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return Array.from(new Uint8Array(digest))
    .map((b) => b.toString(16).padStart(2, "0").toUpperCase())
    .join(":");
}

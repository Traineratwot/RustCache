/**
 * Typed HTTP client for the RustCache REST API.
 * Every call returns the parsed body or throws `ApiError`.
 */
import { ApiError, parseApiError } from "./errors";
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
  if (!r.ok) throw await parseApiError(url, r);
  return r.json();
}

async function sendJson<T>(url: string, method: string, body?: unknown): Promise<T> {
  const r = await fetch(url, {
    method,
    headers: body !== undefined ? { "Content-Type": "application/json" } : undefined,
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
  if (!r.ok) throw await parseApiError(url, r);
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
 * Save the full TOML config.
 *
 * Validation failures are returned as a result object (`ok: false` +
 * `errors[]`) rather than thrown, so the Settings form can highlight fields.
 * Transport/parse failures still throw `ApiError`.
 */
export async function updateConfig(body: Config): Promise<ConfigUpdateResult> {
  const r = await fetch("/api/config", {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  const text = await r.text();
  let parsed: unknown = {};
  try {
    parsed = text ? JSON.parse(text) : {};
  } catch {
    throw new ApiError(`config ${r.status}`, r.status, "/api/config");
  }
  return parsed as ConfigUpdateResult;
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

/** Download the root CA PEM. Throws `ApiError` on failure. */
export async function downloadCa(): Promise<string> {
  const r = await fetch("/api/ca.crt");
  if (!r.ok) throw await parseApiError("/api/ca.crt", r);
  return r.text();
}

export type { ApiError, FieldIssue } from "./errors";

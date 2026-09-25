export interface Stats {
  hits: number;
  misses: number;
  revalidations: number;
  bypasses: number;
  bytes_served: number;
  bytes_saved: number;
  errors: number;
  tunnels: number;
  hit_rate: number;
  saved_mb: number;
}

export interface ReqRecord {
  ts: number;
  method: string;
  url: string;
  host: string;
  status: number;
  outcome: string;
  duration_ms: number;
  resp_bytes: number;
}

export interface RequestQuery {
  q?: string;
  method?: string;
  outcome?: string;
  status_min?: number;
  status_max?: number;
  since?: number;
  until?: number;
  limit?: number;
  offset?: number;
}

export interface RequestPage {
  requests: ReqRecord[];
  total: number;
  limit: number;
  offset: number;
}

export interface LogSettings {
  max_rows: number;
  max_age_days: number;
  cleanup_interval_secs: number;
}

export interface Matcher {
  kind: "exact" | "wildcard" | "suffix" | "cidr";
  value: string;
}

export type PacMode = "http" | "socks" | "http+socks";

export interface Config {
  data_dir: string;
  http: { port: number };
  https: { port: number };
  socks5: { port: number };
  api: { bind: string };
  cache: {
    dir: string;
    max_bytes: number;
    max_object_bytes: number;
  };
  exclude: {
    domains: string[];
    cidrs: string[];
  };
  ca: { dir: string };
  pac: {
    enabled: boolean;
    bind: string;
    mode: PacMode;
  };
  logs: {
    db_path: string;
    max_rows: number;
    max_age_days: number;
    cleanup_interval_secs: number;
  };
}

export interface PacInfo {
  enabled: boolean;
  mode: PacMode;
  bind: string;
  port: number;
  urls: string[];
}

export interface CacheInfo {
  bytes: number;
  entries: number;
}

export interface NetInfo {
  local_ips: string[];
}

export interface ListenerInfo {
  name: string;
  bind: string;
  port: number;
  running: boolean;
}

export interface HealthInfo {
  ok: boolean;
  uptime_s: number;
  listeners: ListenerInfo[];
}

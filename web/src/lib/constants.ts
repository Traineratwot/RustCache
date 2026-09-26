/**
 * Shared UI numeric constants and poll intervals.
 * Keeps magic numbers out of page components and documents unit intent.
 */

/** Bytes in one mebibyte (used for cache size form fields). */
export const MIB = 1024 * 1024;

/** Upper bound accepted by the API for `logs.max_rows`. */
export const MAX_LOG_ROWS = 10_000_000;

/** Upper bound accepted by the API for `logs.max_age_days`. */
export const MAX_LOG_AGE_DAYS = 3650;

/** Upper bound accepted by the API for `logs.cleanup_interval_secs`. */
export const MAX_CLEANUP_INTERVAL_SECS = 86_400;

/** Lower bound accepted by the API for `logs.cleanup_interval_secs`. */
export const MIN_CLEANUP_INTERVAL_SECS = 10;

/** Default poll interval for request-log auto-refresh (ms). */
export const REQUESTS_POLL_MS = 2000;

/** Default poll interval for health page auto-refresh (ms). */
export const HEALTH_POLL_MS = 3000;

/** Default poll interval for cache page auto-refresh (ms). */
export const CACHE_POLL_MS = 5000;

/** How often Settings polls the API after a process restart (ms). */
export const RESTART_POLL_MS = 500;

/** How many times Settings polls after restart before giving up. */
export const RESTART_POLL_ATTEMPTS = 30;

/** Fallback listener ports shown when live config is unavailable. */
export const DEFAULT_PORTS = {
  http: 3128,
  https: 3129,
  socks5: 1080,
  api: 8080,
  pac: 8081,
} as const;

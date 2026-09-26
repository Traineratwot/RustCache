//! Request-log data types exchanged between the writer thread, queries, and the HTTP API.

use serde::{Deserialize, Serialize};

use super::outcome::Outcome;

/// One request as recorded in the traffic history (UI + `/api/requests`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReqRecord {
    /// Event timestamp, milliseconds since the Unix epoch.
    pub ts: u64,
    /// HTTP method as written on the wire (e.g. `"GET"`).
    pub method: String,
    /// Full request URL.
    pub url: String,
    /// Host the request was sent to.
    pub host: String,
    /// HTTP status code.
    pub status: u16,
    /// Cache outcome. Wire format is the historical uppercase label ("HIT", …).
    pub outcome: Outcome,
    /// Request duration, milliseconds.
    pub duration_ms: u64,
    /// Response body size, bytes.
    pub resp_bytes: u64,
}

/// Filter + pagination for querying the request log.
#[derive(Debug, Clone, Default)]
pub struct LogQuery {
    /// Free-text substring match against URL and host (case-insensitive).
    pub q: Option<String>,
    /// Exact method filter.
    pub method: Option<String>,
    /// Exact outcome filter.
    pub outcome: Option<Outcome>,
    /// Inclusive lower bound on the HTTP status code.
    pub status_min: Option<u16>,
    /// Inclusive upper bound on the HTTP status code.
    pub status_max: Option<u16>,
    /// Inclusive lower bound on the event timestamp (ms).
    pub since_ms: Option<u64>,
    /// Inclusive upper bound on the event timestamp (ms).
    pub until_ms: Option<u64>,
    /// Page size.
    pub limit: u32,
    /// Rows to skip for pagination.
    pub offset: u32,
}

/// One page of request-log results.
#[derive(Debug, Clone, Serialize)]
pub struct LogPage {
    /// Matching rows on this page, newest first.
    pub requests: Vec<ReqRecord>,
    /// Total matching rows across all pages.
    pub total: u64,
}

/// Time window for aggregate stats.
#[derive(Debug, Clone, Default)]
pub struct LogStatsQuery {
    /// Inclusive lower bound on the event timestamp (ms).
    pub since_ms: Option<u64>,
    /// Inclusive upper bound on the event timestamp (ms).
    pub until_ms: Option<u64>,
}

/// Per-outcome aggregate (always includes every [`Outcome::ALL`] label).
#[derive(Debug, Clone, Serialize)]
pub struct OutcomeStat {
    /// Outcome this row aggregates.
    pub outcome: Outcome,
    /// Number of requests with this outcome.
    pub count: u64,
    /// Sum of response body bytes.
    pub bytes: u64,
    /// Mean request duration, milliseconds.
    pub avg_duration_ms: f64,
}

/// Per-host aggregate (top hosts by request count).
#[derive(Debug, Clone, Serialize)]
pub struct HostStat {
    /// Host name.
    pub host: String,
    /// Number of requests to this host.
    pub count: u64,
    /// Sum of response body bytes.
    pub bytes: u64,
    /// Requests served from cache.
    pub hits: u64,
    /// `hits / count` (0 when `count` is 0).
    pub hit_rate: f64,
}

/// One time bucket of the request-count series.
#[derive(Debug, Clone, Serialize)]
pub struct SeriesPoint {
    /// Bucket start timestamp (ms), aligned to `LogStats::bucket_ms`.
    pub ts: u64,
    /// Requests in this bucket.
    pub count: u64,
    /// Requests served from cache.
    pub hits: u64,
    /// Requests counted as cache misses.
    pub miss_like: u64,
    /// Sum of response body bytes.
    pub bytes: u64,
}

/// Aggregated request-log statistics (persisted history, not process counters).
#[derive(Debug, Clone, Serialize)]
pub struct LogStats {
    /// Earliest timestamp inside the filtered window.
    pub since_ms: u64,
    /// Latest timestamp inside the filtered window.
    pub until_ms: u64,
    /// Total requests in the window.
    pub total: u64,
    /// Requests served from cache.
    pub hits: u64,
    /// Requests counted as cache misses.
    pub miss_like: u64,
    /// `hits / (hits + miss_like)` (0 when the denominator is 0).
    pub hit_rate: f64,
    /// Sum of response body bytes.
    pub bytes_served: u64,
    /// Body bytes served from cache instead of the origin.
    pub bytes_saved: u64,
    /// `bytes_saved` in mebibytes.
    pub saved_mb: f64,
    /// Mean request duration, milliseconds.
    pub avg_duration_ms: f64,
    /// Slowest request duration, milliseconds.
    pub max_duration_ms: u64,
    /// Series bucket width, milliseconds.
    pub bucket_ms: u64,
    /// Per-outcome aggregates, canonical labels always present (zero-filled).
    pub by_outcome: Vec<OutcomeStat>,
    /// Busiest hosts, capped at 10.
    pub top_hosts: Vec<HostStat>,
    /// Gap-filled time series over the window.
    pub series: Vec<SeriesPoint>,
}

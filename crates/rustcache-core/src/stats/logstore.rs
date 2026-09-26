//! Persistent request-log store backed by SQLite.

use std::path::Path;
use std::sync::mpsc::{channel, sync_channel, SyncSender};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

const CHANNEL_CAP: usize = 8192;
const INSERT_TRIM_EVERY: u64 = 256;
const DEFAULT_MAX_ROWS: u64 = 10_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReqRecord {
    pub ts: u64,
    pub method: String,
    pub url: String,
    pub host: String,
    pub status: u16,
    pub outcome: String,
    pub duration_ms: u64,
    pub resp_bytes: u64,
}

#[derive(Debug, Clone, Default)]
pub struct LogQuery {
    pub q: Option<String>,
    pub method: Option<String>,
    pub outcome: Option<String>,
    pub status_min: Option<u16>,
    pub status_max: Option<u16>,
    pub since_ms: Option<u64>,
    pub until_ms: Option<u64>,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogPage {
    pub requests: Vec<ReqRecord>,
    pub total: u64,
}

#[derive(Debug, Clone, Default)]
pub struct LogStatsQuery {
    pub since_ms: Option<u64>,
    pub until_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OutcomeStat {
    pub outcome: String,
    pub count: u64,
    pub bytes: u64,
    pub avg_duration_ms: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct HostStat {
    pub host: String,
    pub count: u64,
    pub bytes: u64,
    pub hits: u64,
    pub hit_rate: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SeriesPoint {
    pub ts: u64,
    pub count: u64,
    pub hits: u64,
    pub miss_like: u64,
    pub bytes: u64,
}

/// Aggregated request-log statistics (persisted history, not process counters).
#[derive(Debug, Clone, Serialize)]
pub struct LogStats {
    pub since_ms: u64,
    pub until_ms: u64,
    pub total: u64,
    pub hits: u64,
    pub miss_like: u64,
    pub hit_rate: f64,
    pub bytes_served: u64,
    pub bytes_saved: u64,
    pub saved_mb: f64,
    pub avg_duration_ms: f64,
    pub max_duration_ms: u64,
    pub bucket_ms: u64,
    pub by_outcome: Vec<OutcomeStat>,
    pub top_hosts: Vec<HostStat>,
    pub series: Vec<SeriesPoint>,
}

enum LogCmd {
    Insert(ReqRecord),
    Query(LogQuery, std::sync::mpsc::Sender<anyhow::Result<LogPage>>),
    Stats(
        LogStatsQuery,
        std::sync::mpsc::Sender<anyhow::Result<LogStats>>,
    ),
    Clear(std::sync::mpsc::Sender<anyhow::Result<u64>>),
    Cleanup {
        max_rows: u64,
        max_age_days: u64,
        reply: std::sync::mpsc::Sender<anyhow::Result<(u64, u64)>>,
    },
    Flush(std::sync::mpsc::Sender<()>),
}

struct WriterState {
    conn: Connection,
    inserts_since_trim: u64,
    max_rows: u64,
    cleanup_runs: u64,
}

#[derive(Clone)]
pub struct LogStore {
    tx: SyncSender<LogCmd>,
}

impl LogStore {
    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let conn = Connection::open(path)?;
        set_db_mode(path);
        init_schema(&conn)?;
        let (tx, rx) = sync_channel::<LogCmd>(CHANNEL_CAP);
        let mut state = WriterState {
            conn,
            inserts_since_trim: 0,
            max_rows: DEFAULT_MAX_ROWS,
            cleanup_runs: 0,
        };
        std::thread::Builder::new()
            .name("rustcache-log-writer".into())
            .spawn(move || {
                while let Ok(cmd) = rx.recv() {
                    handle_cmd(&mut state, cmd);
                }
            })
            .map_err(|e| anyhow::anyhow!("spawn log writer: {e}"))?;
        Ok(Self { tx })
    }

    /// Non-blocking enqueue. Drops the record if the channel is full.
    pub fn enqueue(&self, rec: ReqRecord) {
        if self.tx.try_send(LogCmd::Insert(rec)).is_err() {
            tracing::warn!("request log channel full, dropping record");
        }
    }

    pub async fn query(&self, q: LogQuery) -> anyhow::Result<LogPage> {
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let (rtx, rrx) = channel();
            tx.send(LogCmd::Query(q, rtx))
                .map_err(|_| anyhow::anyhow!("log writer gone"))?;
            recv_result(rrx)
        })
        .await
        .map_err(|e| anyhow::anyhow!("log query join: {e}"))?
    }

    pub async fn stats(&self, q: LogStatsQuery) -> anyhow::Result<LogStats> {
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let (rtx, rrx) = channel();
            tx.send(LogCmd::Stats(q, rtx))
                .map_err(|_| anyhow::anyhow!("log writer gone"))?;
            recv_result(rrx)
        })
        .await
        .map_err(|e| anyhow::anyhow!("log stats join: {e}"))?
    }

    pub async fn clear(&self) -> anyhow::Result<u64> {
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let (rtx, rrx) = channel();
            tx.send(LogCmd::Clear(rtx))
                .map_err(|_| anyhow::anyhow!("log writer gone"))?;
            recv_result(rrx)
        })
        .await
        .map_err(|e| anyhow::anyhow!("log clear join: {e}"))?
    }

    pub async fn cleanup(&self, max_rows: u64, max_age_days: u64) -> anyhow::Result<(u64, u64)> {
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let (rtx, rrx) = channel();
            tx.send(LogCmd::Cleanup {
                max_rows,
                max_age_days,
                reply: rtx,
            })
            .map_err(|_| anyhow::anyhow!("log writer gone"))?;
            recv_result(rrx)
        })
        .await
        .map_err(|e| anyhow::anyhow!("log cleanup join: {e}"))?
    }

    /// Barrier: returns after all previously enqueued commands are processed.
    pub async fn flush(&self) -> anyhow::Result<()> {
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let (rtx, rrx) = channel();
            tx.send(LogCmd::Flush(rtx))
                .map_err(|_| anyhow::anyhow!("log writer gone"))?;
            rrx.recv()
                .map_err(|_| anyhow::anyhow!("log flush reply dropped"))
        })
        .await
        .map_err(|e| anyhow::anyhow!("log flush join: {e}"))?
    }
}

fn recv_result<T>(rx: std::sync::mpsc::Receiver<anyhow::Result<T>>) -> anyhow::Result<T> {
    match rx.recv() {
        Ok(r) => r,
        Err(_) => anyhow::bail!("log writer dropped reply"),
    }
}

fn set_db_mode(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o600);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

fn init_schema(conn: &Connection) -> anyhow::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS requests (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            ts          INTEGER NOT NULL,
            method      TEXT    NOT NULL,
            url         TEXT    NOT NULL,
            host        TEXT    NOT NULL,
            status      INTEGER NOT NULL,
            outcome     TEXT    NOT NULL,
            duration_ms INTEGER NOT NULL,
            resp_bytes  INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_requests_ts      ON requests(ts DESC);
        CREATE INDEX IF NOT EXISTS idx_requests_outcome ON requests(outcome);
        CREATE INDEX IF NOT EXISTS idx_requests_host    ON requests(host);
        CREATE INDEX IF NOT EXISTS idx_requests_method  ON requests(method);
        "#,
    )?;
    Ok(())
}

fn handle_cmd(state: &mut WriterState, cmd: LogCmd) {
    match cmd {
        LogCmd::Insert(rec) => {
            if let Err(e) = insert(&state.conn, &rec) {
                tracing::warn!(error = %e, "log insert failed");
            }
            state.inserts_since_trim += 1;
            if state.inserts_since_trim >= INSERT_TRIM_EVERY {
                state.inserts_since_trim = 0;
                let max_rows = state.max_rows;
                if let Err(e) = trim_rows(&state.conn, max_rows) {
                    tracing::warn!(error = %e, "log trim failed");
                }
            }
        }
        LogCmd::Query(q, reply) => {
            let _ = reply.send(query(&state.conn, &q));
        }
        LogCmd::Stats(q, reply) => {
            let _ = reply.send(stats(&state.conn, &q));
        }
        LogCmd::Clear(reply) => {
            let _ = reply.send(clear(&state.conn));
        }
        LogCmd::Cleanup {
            max_rows,
            max_age_days,
            reply,
        } => {
            state.max_rows = max_rows.max(1);
            let _ = reply.send(cleanup(&state.conn, max_rows, max_age_days));
            state.cleanup_runs += 1;
            if state.cleanup_runs.is_multiple_of(10) {
                let _ = state.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
            }
        }
        LogCmd::Flush(reply) => {
            let _ = reply.send(());
        }
    }
}

fn insert(conn: &Connection, rec: &ReqRecord) -> anyhow::Result<()> {
    conn.execute(
        r#"INSERT INTO requests (ts, method, url, host, status, outcome, duration_ms, resp_bytes)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"#,
        params![
            rec.ts as i64,
            rec.method,
            rec.url,
            rec.host,
            rec.status as i64,
            rec.outcome,
            rec.duration_ms as i64,
            rec.resp_bytes as i64,
        ],
    )?;
    Ok(())
}

fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' | '%' | '_' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

struct FilterSql {
    where_sql: String,
    params: Vec<rusqlite::types::Value>,
}

fn build_filter(q: &LogQuery) -> FilterSql {
    use rusqlite::types::Value;

    let mut where_sql = String::from(" WHERE 1=1");
    let mut params: Vec<Value> = Vec::new();

    if let Some(text) = q.q.as_deref().filter(|s| !s.is_empty()) {
        let pat = format!("%{}%", escape_like(&text.to_lowercase()));
        where_sql
            .push_str(" AND (LOWER(url) LIKE ? ESCAPE '\\' OR LOWER(host) LIKE ? ESCAPE '\\')");
        params.push(Value::Text(pat.clone()));
        params.push(Value::Text(pat));
    }
    if let Some(m) = q.method.as_deref().filter(|s| !s.is_empty()) {
        where_sql.push_str(" AND method = ?");
        params.push(Value::Text(m.to_string()));
    }
    if let Some(o) = q.outcome.as_deref().filter(|s| !s.is_empty()) {
        where_sql.push_str(" AND outcome = ?");
        params.push(Value::Text(o.to_string()));
    }
    if let Some(min) = q.status_min {
        where_sql.push_str(" AND status >= ?");
        params.push(Value::Integer(min as i64));
    }
    if let Some(max) = q.status_max {
        where_sql.push_str(" AND status <= ?");
        params.push(Value::Integer(max as i64));
    }
    if let Some(since) = q.since_ms {
        where_sql.push_str(" AND ts >= ?");
        params.push(Value::Integer(since as i64));
    }
    if let Some(until) = q.until_ms {
        where_sql.push_str(" AND ts <= ?");
        params.push(Value::Integer(until as i64));
    }

    FilterSql { where_sql, params }
}

fn query(conn: &Connection, q: &LogQuery) -> anyhow::Result<LogPage> {
    let filter = build_filter(q);
    let limit = q.limit.clamp(1, 500) as i64;
    let offset = q.offset as i64;

    let count_sql = format!("SELECT COUNT(*) FROM requests{}", filter.where_sql);
    let total: i64 = conn.query_row(
        &count_sql,
        rusqlite::params_from_iter(&filter.params),
        |r| r.get(0),
    )?;

    let page_sql = format!(
        "SELECT ts, method, url, host, status, outcome, duration_ms, resp_bytes \
         FROM requests{} ORDER BY ts DESC, id DESC LIMIT ? OFFSET ?",
        filter.where_sql
    );
    let mut stmt = conn.prepare(&page_sql)?;
    let mut sql_params = filter.params.clone();
    sql_params.push(rusqlite::types::Value::Integer(limit));
    sql_params.push(rusqlite::types::Value::Integer(offset));
    let rows = stmt.query_map(rusqlite::params_from_iter(&sql_params), |r| {
        Ok(ReqRecord {
            ts: r.get::<_, i64>(0)? as u64,
            method: r.get(1)?,
            url: r.get(2)?,
            host: r.get(3)?,
            status: r.get::<_, i64>(4)? as u16,
            outcome: r.get(5)?,
            duration_ms: r.get::<_, i64>(6)? as u64,
            resp_bytes: r.get::<_, i64>(7)? as u64,
        })
    })?;
    let mut requests = Vec::new();
    for row in rows {
        requests.push(row?);
    }

    Ok(LogPage {
        requests,
        total: total.max(0) as u64,
    })
}

/// Canonical outcome labels, always present in `by_outcome` (zero-filled).
const CANONICAL_OUTCOMES: [&str; 8] = [
    "HIT",
    "HIT_REVALIDATED",
    "REVALIDATED",
    "MISS",
    "BYPASS",
    "TUNNEL",
    "ERROR",
    "REJECT_CMD",
];

const SERIES_TARGET_BUCKETS: u64 = 48;
const DEFAULT_BUCKET_MS: u64 = 60_000;
const BUCKET_LADDER_MS: [u64; 9] = [
    60_000, 300_000, 900_000, 1_800_000, 3_600_000, 10_800_000, 21_600_000, 43_200_000, 86_400_000,
];

fn pick_bucket_ms(span_ms: u64) -> u64 {
    if span_ms == 0 {
        return DEFAULT_BUCKET_MS;
    }
    for step in BUCKET_LADDER_MS {
        if span_ms / step <= SERIES_TARGET_BUCKETS {
            return step;
        }
    }
    86_400_000
}

fn stats(conn: &Connection, q: &LogStatsQuery) -> anyhow::Result<LogStats> {
    let filter = build_filter(&LogQuery {
        since_ms: q.since_ms,
        until_ms: q.until_ms,
        ..Default::default()
    });

    // Q1: totals + span
    let totals_sql = format!(
        "SELECT COUNT(*), \
                COALESCE(SUM(resp_bytes),0), \
                COALESCE(SUM(duration_ms),0), \
                COALESCE(MAX(duration_ms),0), \
                COALESCE(MIN(ts),0), \
                COALESCE(MAX(ts),0), \
                COALESCE(SUM(CASE WHEN outcome IN ('HIT','HIT_REVALIDATED') THEN 1 ELSE 0 END),0), \
                COALESCE(SUM(CASE WHEN outcome IN ('MISS','REVALIDATED') THEN 1 ELSE 0 END),0), \
                COALESCE(SUM(CASE WHEN outcome IN ('HIT','HIT_REVALIDATED') THEN resp_bytes ELSE 0 END),0) \
         FROM requests{}",
        filter.where_sql
    );
    let (
        total,
        bytes_served,
        sum_duration,
        max_duration,
        min_ts,
        max_ts,
        hits,
        miss_like,
        bytes_saved,
    ): (i64, i64, i64, i64, i64, i64, i64, i64, i64) = conn.query_row(
        &totals_sql,
        rusqlite::params_from_iter(&filter.params),
        |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
            ))
        },
    )?;

    let total = total.max(0) as u64;
    let hits = hits.max(0) as u64;
    let miss_like = miss_like.max(0) as u64;
    let hit_rate = if hits + miss_like == 0 {
        0.0
    } else {
        hits as f64 / (hits + miss_like) as f64
    };
    let avg_duration_ms = if total == 0 {
        0.0
    } else {
        sum_duration.max(0) as f64 / total as f64
    };
    let bytes_saved = bytes_saved.max(0) as u64;

    // Q2: by_outcome
    let by_outcome_sql = format!(
        "SELECT outcome, COUNT(*), COALESCE(SUM(resp_bytes),0), COALESCE(AVG(duration_ms),0) \
         FROM requests{} GROUP BY outcome",
        filter.where_sql
    );
    let mut stmt = conn.prepare(&by_outcome_sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(&filter.params), |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, f64>(3)?,
        ))
    })?;
    let mut raw_outcomes: Vec<(String, u64, u64, f64)> = Vec::new();
    for row in rows {
        let (outcome, count, bytes, avg) = row?;
        raw_outcomes.push((outcome, count.max(0) as u64, bytes.max(0) as u64, avg));
    }

    let mut by_outcome: Vec<OutcomeStat> = Vec::with_capacity(CANONICAL_OUTCOMES.len() + 4);
    for canon in CANONICAL_OUTCOMES {
        let found = raw_outcomes.iter().find(|(o, ..)| o == canon);
        match found {
            Some((outcome, count, bytes, avg)) => by_outcome.push(OutcomeStat {
                outcome: outcome.clone(),
                count: *count,
                bytes: *bytes,
                avg_duration_ms: *avg,
            }),
            None => by_outcome.push(OutcomeStat {
                outcome: canon.to_string(),
                count: 0,
                bytes: 0,
                avg_duration_ms: 0.0,
            }),
        }
    }
    let mut extras: Vec<(String, u64, u64, f64)> = raw_outcomes
        .into_iter()
        .filter(|(o, ..)| !CANONICAL_OUTCOMES.contains(&o.as_str()))
        .collect();
    extras.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    for (outcome, count, bytes, avg) in extras {
        by_outcome.push(OutcomeStat {
            outcome,
            count,
            bytes,
            avg_duration_ms: avg,
        });
    }

    // Q3: top_hosts
    let top_hosts_sql = format!(
        "SELECT host, COUNT(*), COALESCE(SUM(resp_bytes),0), \
                COALESCE(SUM(CASE WHEN outcome IN ('HIT','HIT_REVALIDATED') THEN 1 ELSE 0 END),0) \
         FROM requests{} GROUP BY host ORDER BY COUNT(*) DESC, host ASC LIMIT 10",
        filter.where_sql
    );
    let mut stmt = conn.prepare(&top_hosts_sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(&filter.params), |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, i64>(3)?,
        ))
    })?;
    let mut top_hosts = Vec::new();
    for row in rows {
        let (host, count, bytes, host_hits) = row?;
        let count = count.max(0) as u64;
        let host_hits = host_hits.max(0) as u64;
        let hit_rate = if count == 0 {
            0.0
        } else {
            host_hits as f64 / count as f64
        };
        top_hosts.push(HostStat {
            host,
            count,
            bytes: bytes.max(0) as u64,
            hits: host_hits,
            hit_rate,
        });
    }

    // Q4: time series
    let bucket_ms = if total == 0 {
        DEFAULT_BUCKET_MS
    } else {
        pick_bucket_ms(max_ts.max(0) as u64 - min_ts.max(0) as u64)
    };

    let mut series: Vec<SeriesPoint> = Vec::new();
    if total > 0 {
        let series_sql = format!(
            "SELECT (ts / ?) * ? AS b, COUNT(*), \
                    COALESCE(SUM(CASE WHEN outcome IN ('HIT','HIT_REVALIDATED') THEN 1 ELSE 0 END),0), \
                    COALESCE(SUM(CASE WHEN outcome IN ('MISS','REVALIDATED') THEN 1 ELSE 0 END),0), \
                    COALESCE(SUM(resp_bytes),0) \
             FROM requests{} GROUP BY b ORDER BY b",
            filter.where_sql
        );
        // `(ts / ?) * ?` appears before WHERE placeholders, so bind bucket first.
        let mut sql_params = vec![
            rusqlite::types::Value::Integer(bucket_ms as i64),
            rusqlite::types::Value::Integer(bucket_ms as i64),
        ];
        sql_params.extend_from_slice(&filter.params);
        let mut stmt = conn.prepare(&series_sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(&sql_params), |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })?;
        let mut raw_buckets: Vec<(u64, u64, u64, u64, u64)> = Vec::new();
        for row in rows {
            let (ts, count, h, m, b) = row?;
            raw_buckets.push((
                ts.max(0) as u64,
                count.max(0) as u64,
                h.max(0) as u64,
                m.max(0) as u64,
                b.max(0) as u64,
            ));
        }

        // Gap-fill from floor(min/bucket)*bucket to floor(max/bucket)*bucket inclusive.
        let b = bucket_ms.max(1);
        let start = (min_ts.max(0) as u64 / b) * b;
        let end = (max_ts.max(0) as u64 / b) * b;
        let mut cur = start;
        let mut idx = 0usize;
        loop {
            let point = if idx < raw_buckets.len() && raw_buckets[idx].0 == cur {
                let (_, count, h, m, bytes) = raw_buckets[idx];
                idx += 1;
                SeriesPoint {
                    ts: cur,
                    count,
                    hits: h,
                    miss_like: m,
                    bytes,
                }
            } else {
                SeriesPoint {
                    ts: cur,
                    count: 0,
                    hits: 0,
                    miss_like: 0,
                    bytes: 0,
                }
            };
            series.push(point);
            if cur >= end {
                break;
            }
            cur = cur.saturating_add(b);
            if series.len() > 10_000 {
                break;
            }
        }
    }

    Ok(LogStats {
        since_ms: min_ts.max(0) as u64,
        until_ms: max_ts.max(0) as u64,
        total,
        hits,
        miss_like,
        hit_rate,
        bytes_served: bytes_served.max(0) as u64,
        bytes_saved,
        saved_mb: bytes_saved as f64 / (1024.0 * 1024.0),
        avg_duration_ms,
        max_duration_ms: max_duration.max(0) as u64,
        bucket_ms,
        by_outcome,
        top_hosts,
        series,
    })
}

fn clear(conn: &Connection) -> anyhow::Result<u64> {
    let n = conn.execute("DELETE FROM requests", [])?;
    Ok(n as u64)
}

fn trim_rows(conn: &Connection, max_rows: u64) -> anyhow::Result<u64> {
    let max_rows = max_rows.max(1) as i64;
    let n = conn.execute(
        "DELETE FROM requests WHERE id NOT IN (
            SELECT id FROM requests ORDER BY ts DESC, id DESC LIMIT ?1
        )",
        params![max_rows],
    )?;
    Ok(n as u64)
}

fn cleanup(conn: &Connection, max_rows: u64, max_age_days: u64) -> anyhow::Result<(u64, u64)> {
    let now = rustcache_now_ms();
    let cutoff = now.saturating_sub(max_age_days.saturating_mul(86_400_000)) as i64;
    let by_age = conn.execute("DELETE FROM requests WHERE ts < ?1", params![cutoff])? as u64;
    let by_rows = trim_rows(conn, max_rows)?;
    Ok((by_age, by_rows))
}

fn rustcache_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Optional peek used by tests to verify schema objects.
#[cfg(test)]
fn table_exists(conn: &Connection, name: &str) -> anyhow::Result<bool> {
    use rusqlite::OptionalExtension;
    let found: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
            params![name],
            |r| r.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

#[cfg(test)]
fn index_exists(conn: &Connection, name: &str) -> anyhow::Result<bool> {
    use rusqlite::OptionalExtension;
    let found: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='index' AND name=?1",
            params![name],
            |r| r.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    struct TempDb {
        dir: PathBuf,
    }

    impl TempDb {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static N: AtomicU64 = AtomicU64::new(0);
            let n = N.fetch_add(1, Ordering::SeqCst);
            let dir = std::env::temp_dir().join(format!("rc-logstore-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("tempdir");
            Self { dir }
        }
        fn path(&self) -> PathBuf {
            self.dir.join("logs.db")
        }
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn rec(ts: u64, method: &str, url: &str, host: &str, status: u16, outcome: &str) -> ReqRecord {
        ReqRecord {
            ts,
            method: method.into(),
            url: url.into(),
            host: host.into(),
            status,
            outcome: outcome.into(),
            duration_ms: ts,
            resp_bytes: 10,
        }
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("rt")
    }

    #[test]
    fn open_creates_schema() {
        let db = TempDb::new();
        let store = LogStore::open(db.path()).expect("open");
        let _ = &store;
        let conn = Connection::open(db.path()).expect("reopen");
        assert!(table_exists(&conn, "requests").unwrap());
        assert!(index_exists(&conn, "idx_requests_ts").unwrap());
        assert!(index_exists(&conn, "idx_requests_outcome").unwrap());
        // second open is idempotent
        LogStore::open(db.path()).expect("reopen2");
    }

    #[test]
    fn insert_and_query_newest_first() {
        let db = TempDb::new();
        let store = LogStore::open(db.path()).expect("open");
        let rt = rt();
        rt.block_on(async {
            for i in 0..5 {
                store.enqueue(rec(i, "GET", &format!("http://e/{i}"), "e", 200, "HIT"));
            }
            store.flush().await.expect("flush");
            let page = store
                .query(LogQuery {
                    limit: 10,
                    ..Default::default()
                })
                .await
                .expect("query");
            assert_eq!(page.total, 5);
            assert_eq!(page.requests.len(), 5);
            assert_eq!(page.requests[0].ts, 4);
            assert_eq!(page.requests[4].ts, 0);
        });
    }

    #[test]
    fn filter_by_q_matches_url_and_host() {
        let db = TempDb::new();
        let store = LogStore::open(db.path()).expect("open");
        let rt = rt();
        rt.block_on(async {
            store.enqueue(rec(
                1,
                "GET",
                "http://example.com/a",
                "example.com",
                200,
                "HIT",
            ));
            store.enqueue(rec(
                2,
                "GET",
                "http://other.test/b",
                "other.test",
                200,
                "MISS",
            ));
            store.enqueue(rec(3, "GET", "http://x/pct%100", "x", 200, "HIT"));
            store.enqueue(rec(4, "GET", "http://x/under_score", "x", 200, "HIT"));
            store.flush().await.expect("flush");

            let page = store
                .query(LogQuery {
                    q: Some("example".into()),
                    limit: 10,
                    ..Default::default()
                })
                .await
                .expect("q");
            assert_eq!(page.total, 1);
            assert_eq!(page.requests[0].host, "example.com");

            // host match
            let page = store
                .query(LogQuery {
                    q: Some("OTHER".into()),
                    limit: 10,
                    ..Default::default()
                })
                .await
                .expect("q2");
            assert_eq!(page.total, 1);

            // % and _ are literal (not LIKE wildcards)
            let page = store
                .query(LogQuery {
                    q: Some("%".into()),
                    limit: 10,
                    ..Default::default()
                })
                .await
                .expect("pct");
            assert_eq!(page.total, 1);
            assert_eq!(page.requests[0].url, "http://x/pct%100");

            let page = store
                .query(LogQuery {
                    q: Some("under_score".into()),
                    limit: 10,
                    ..Default::default()
                })
                .await
                .expect("us");
            assert_eq!(page.total, 1);
            assert_eq!(page.requests[0].url, "http://x/under_score");
        });
    }

    #[test]
    fn filter_by_method_outcome_status_range() {
        let db = TempDb::new();
        let store = LogStore::open(db.path()).expect("open");
        let rt = rt();
        rt.block_on(async {
            store.enqueue(rec(1, "GET", "http://a", "a", 200, "HIT"));
            store.enqueue(rec(2, "POST", "http://b", "b", 404, "MISS"));
            store.enqueue(rec(3, "GET", "http://c", "c", 500, "ERROR"));
            store.flush().await.expect("flush");

            let page = store
                .query(LogQuery {
                    method: Some("GET".into()),
                    limit: 10,
                    ..Default::default()
                })
                .await
                .expect("m");
            assert_eq!(page.total, 2);

            let page = store
                .query(LogQuery {
                    outcome: Some("MISS".into()),
                    limit: 10,
                    ..Default::default()
                })
                .await
                .expect("o");
            assert_eq!(page.total, 1);

            let page = store
                .query(LogQuery {
                    status_min: Some(400),
                    status_max: Some(499),
                    limit: 10,
                    ..Default::default()
                })
                .await
                .expect("s");
            assert_eq!(page.total, 1);
            assert_eq!(page.requests[0].status, 404);

            let page = store
                .query(LogQuery {
                    method: Some("GET".into()),
                    outcome: Some("HIT".into()),
                    status_min: Some(200),
                    status_max: Some(299),
                    limit: 10,
                    ..Default::default()
                })
                .await
                .expect("combo");
            assert_eq!(page.total, 1);
        });
    }

    #[test]
    fn filter_by_time_range() {
        let db = TempDb::new();
        let store = LogStore::open(db.path()).expect("open");
        let rt = rt();
        rt.block_on(async {
            store.enqueue(rec(100, "GET", "http://a", "a", 200, "HIT"));
            store.enqueue(rec(200, "GET", "http://b", "b", 200, "HIT"));
            store.enqueue(rec(300, "GET", "http://c", "c", 200, "HIT"));
            store.flush().await.expect("flush");

            let page = store
                .query(LogQuery {
                    since_ms: Some(200),
                    until_ms: Some(200),
                    limit: 10,
                    ..Default::default()
                })
                .await
                .expect("range");
            assert_eq!(page.total, 1);
            assert_eq!(page.requests[0].ts, 200);
        });
    }

    #[test]
    fn pagination_total_and_offset() {
        let db = TempDb::new();
        let store = LogStore::open(db.path()).expect("open");
        let rt = rt();
        rt.block_on(async {
            for i in 0..10 {
                store.enqueue(rec(i, "GET", &format!("http://e/{i}"), "e", 200, "HIT"));
            }
            store.flush().await.expect("flush");

            let p1 = store
                .query(LogQuery {
                    limit: 3,
                    offset: 0,
                    ..Default::default()
                })
                .await
                .expect("p1");
            let p2 = store
                .query(LogQuery {
                    limit: 3,
                    offset: 3,
                    ..Default::default()
                })
                .await
                .expect("p2");
            assert_eq!(p1.total, 10);
            assert_eq!(p2.total, 10);
            assert_eq!(p1.requests.len(), 3);
            assert_eq!(p2.requests.len(), 3);
            assert_eq!(p1.requests[0].ts, 9);
            assert_eq!(p2.requests[0].ts, 6);
            assert_ne!(p1.requests[0].ts, p2.requests[0].ts);
        });
    }

    #[test]
    fn cleanup_max_age() {
        let db = TempDb::new();
        let store = LogStore::open(db.path()).expect("open");
        let rt = rt();
        rt.block_on(async {
            let now = rustcache_now_ms();
            let old = now.saturating_sub(10 * 86_400_000);
            store.enqueue(rec(old, "GET", "http://old", "old", 200, "HIT"));
            store.enqueue(rec(now, "GET", "http://new", "new", 200, "HIT"));
            store.flush().await.expect("flush");

            let (by_age, _by_rows) = store.cleanup(1000, 7).await.expect("cleanup");
            assert_eq!(by_age, 1);
            let page = store
                .query(LogQuery {
                    limit: 10,
                    ..Default::default()
                })
                .await
                .expect("q");
            assert_eq!(page.total, 1);
            assert_eq!(page.requests[0].host, "new");
        });
    }

    #[test]
    fn cleanup_max_rows() {
        let db = TempDb::new();
        let store = LogStore::open(db.path()).expect("open");
        let rt = rt();
        rt.block_on(async {
            let now = rustcache_now_ms();
            for i in 0..10 {
                store.enqueue(rec(
                    now + i,
                    "GET",
                    &format!("http://e/{i}"),
                    "e",
                    200,
                    "HIT",
                ));
            }
            store.flush().await.expect("flush");
            let (by_age, by_rows) = store.cleanup(3, 3650).await.expect("cleanup");
            assert_eq!(by_age, 0);
            assert_eq!(by_rows, 7);
            let page = store
                .query(LogQuery {
                    limit: 10,
                    ..Default::default()
                })
                .await
                .expect("q");
            assert_eq!(page.total, 3);
            assert_eq!(page.requests[0].ts, now + 9);
            assert_eq!(page.requests[2].ts, now + 7);
        });
    }

    #[test]
    fn clear_returns_deleted_count() {
        let db = TempDb::new();
        let store = LogStore::open(db.path()).expect("open");
        let rt = rt();
        rt.block_on(async {
            for i in 0..4 {
                store.enqueue(rec(i, "GET", "http://e", "e", 200, "HIT"));
            }
            store.flush().await.expect("flush");
            let deleted = store.clear().await.expect("clear");
            assert_eq!(deleted, 4);
            let page = store
                .query(LogQuery {
                    limit: 10,
                    ..Default::default()
                })
                .await
                .expect("q");
            assert_eq!(page.total, 0);
        });
    }

    #[test]
    fn enqueue_is_nonblocking_and_lossless_when_idle() {
        let db = TempDb::new();
        let store = LogStore::open(db.path()).expect("open");
        let rt = rt();
        rt.block_on(async {
            for i in 0..100 {
                store.enqueue(rec(i, "GET", "http://e", "e", 200, "HIT"));
            }
            store.flush().await.expect("flush");
            let page = store
                .query(LogQuery {
                    limit: 10,
                    ..Default::default()
                })
                .await
                .expect("q");
            assert_eq!(page.total, 100);
        });
    }

    #[test]
    fn db_file_mode_is_0600() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let db = TempDb::new();
            let _store = LogStore::open(db.path()).expect("open");
            let mode = std::fs::metadata(db.path())
                .expect("meta")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let _ = Duration::from_millis(0);
    }

    fn rec_full(
        ts: u64,
        host: &str,
        outcome: &str,
        duration_ms: u64,
        resp_bytes: u64,
    ) -> ReqRecord {
        ReqRecord {
            ts,
            method: "GET".into(),
            url: format!("http://{host}/"),
            host: host.into(),
            status: 200,
            outcome: outcome.into(),
            duration_ms,
            resp_bytes,
        }
    }

    #[test]
    fn stats_empty_log() {
        let db = TempDb::new();
        let store = LogStore::open(db.path()).expect("open");
        let rt = rt();
        rt.block_on(async {
            store.flush().await.expect("flush");
            let s = store.stats(LogStatsQuery::default()).await.expect("stats");
            assert_eq!(s.total, 0);
            assert_eq!(s.hits, 0);
            assert_eq!(s.miss_like, 0);
            assert_eq!(s.hit_rate, 0.0);
            assert_eq!(s.bytes_served, 0);
            assert_eq!(s.bytes_saved, 0);
            assert_eq!(s.avg_duration_ms, 0.0);
            assert_eq!(s.max_duration_ms, 0);
            assert_eq!(s.by_outcome.len(), 8);
            assert!(s.top_hosts.is_empty());
            assert!(s.series.is_empty());
            assert_eq!(s.bucket_ms, 60_000);
        });
    }

    #[test]
    fn stats_totals_and_hit_mapping() {
        let db = TempDb::new();
        let store = LogStore::open(db.path()).expect("open");
        let rt = rt();
        rt.block_on(async {
            store.enqueue(rec_full(1000, "a", "HIT", 10, 100));
            store.enqueue(rec_full(2000, "a", "HIT", 20, 200));
            store.enqueue(rec_full(3000, "a", "HIT_REVALIDATED", 30, 300));
            store.enqueue(rec_full(4000, "a", "MISS", 40, 400));
            store.enqueue(rec_full(5000, "a", "REVALIDATED", 50, 500));
            store.enqueue(rec_full(6000, "a", "ERROR", 60, 600));
            store.flush().await.expect("flush");

            let s = store.stats(LogStatsQuery::default()).await.expect("stats");
            assert_eq!(s.total, 6);
            assert_eq!(s.hits, 3);
            assert_eq!(s.miss_like, 2);
            assert!((s.hit_rate - 0.6).abs() < 1e-9);
            assert_eq!(s.bytes_saved, 100 + 200 + 300);
            assert_eq!(s.bytes_served, 100 + 200 + 300 + 400 + 500 + 600);
            assert_eq!(s.max_duration_ms, 60);
            assert!((s.avg_duration_ms - (10 + 20 + 30 + 40 + 50 + 60) as f64 / 6.0).abs() < 1e-9);
        });
    }

    #[test]
    fn stats_by_outcome_zero_fill() {
        let db = TempDb::new();
        let store = LogStore::open(db.path()).expect("open");
        let rt = rt();
        rt.block_on(async {
            store.enqueue(rec_full(1000, "a", "HIT", 1, 10));
            store.enqueue(rec_full(2000, "a", "MISS", 1, 10));
            store.flush().await.expect("flush");

            let s = store.stats(LogStatsQuery::default()).await.expect("stats");
            assert_eq!(s.by_outcome.len(), 8);
            assert_eq!(s.by_outcome[0].outcome, "HIT");
            assert_eq!(s.by_outcome[0].count, 1);
            assert_eq!(s.by_outcome[3].outcome, "MISS");
            assert_eq!(s.by_outcome[3].count, 1);
            assert_eq!(s.by_outcome[1].count, 0);
            assert_eq!(s.by_outcome[7].outcome, "REJECT_CMD");
            assert_eq!(s.by_outcome[7].count, 0);
        });
    }

    #[test]
    fn stats_top_hosts() {
        let db = TempDb::new();
        let store = LogStore::open(db.path()).expect("open");
        let rt = rt();
        rt.block_on(async {
            for i in 0..12 {
                let host = format!("h{i}.test");
                // h0 gets 12-i requests so order is h0, h1, ...
                for _ in 0..(12 - i) {
                    store.enqueue(rec_full(1000, &host, "HIT", 1, 5));
                }
            }
            store.flush().await.expect("flush");

            let s = store.stats(LogStatsQuery::default()).await.expect("stats");
            assert_eq!(s.top_hosts.len(), 10);
            assert_eq!(s.top_hosts[0].host, "h0.test");
            assert_eq!(s.top_hosts[0].count, 12);
            assert_eq!(s.top_hosts[0].hits, 12);
            assert!((s.top_hosts[0].hit_rate - 1.0).abs() < 1e-9);
            assert_eq!(s.top_hosts[9].host, "h9.test");
        });
    }

    #[test]
    fn stats_series_bucketing_and_gap_fill() {
        let db = TempDb::new();
        let store = LogStore::open(db.path()).expect("open");
        let rt = rt();
        rt.block_on(async {
            // t=0 and t=10min; span=600_000 → bucket 60s → 11 buckets
            store.enqueue(rec_full(0, "a", "HIT", 1, 10));
            store.enqueue(rec_full(600_000, "a", "MISS", 1, 10));
            store.flush().await.expect("flush");

            let s = store.stats(LogStatsQuery::default()).await.expect("stats");
            assert_eq!(s.bucket_ms, 60_000);
            assert_eq!(s.series.len(), 11);
            assert_eq!(s.series[0].ts, 0);
            assert_eq!(s.series[0].count, 1);
            assert_eq!(s.series[0].hits, 1);
            assert_eq!(s.series[10].ts, 600_000);
            assert_eq!(s.series[10].count, 1);
            assert_eq!(s.series[10].miss_like, 1);
            for p in &s.series[1..10] {
                assert_eq!(p.count, 0);
            }
            for p in &s.series {
                assert_eq!(p.ts % 60_000, 0);
            }
        });
    }

    #[test]
    fn stats_since_until_filter() {
        let db = TempDb::new();
        let store = LogStore::open(db.path()).expect("open");
        let rt = rt();
        rt.block_on(async {
            store.enqueue(rec_full(1000, "a", "HIT", 1, 10));
            store.enqueue(rec_full(2000, "b", "MISS", 1, 10));
            store.enqueue(rec_full(3000, "c", "ERROR", 1, 10));
            store.flush().await.expect("flush");

            let s = store
                .stats(LogStatsQuery {
                    since_ms: Some(2000),
                    until_ms: Some(2000),
                })
                .await
                .expect("stats");
            assert_eq!(s.total, 1);
            assert_eq!(s.since_ms, 2000);
            assert_eq!(s.until_ms, 2000);
            assert_eq!(s.series.len(), 1);
        });
    }
}

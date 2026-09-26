//! Persistent request-log store backed by SQLite.
//!
//! [`LogStore`] is the public facade: commands are queued to a dedicated writer
//! thread and answered over reply channels. Submodules hold the DTOs, schema
//! bootstrap, SQL row operations, aggregate analytics, and the writer loop.

mod analytics;
mod dto;
mod outcome;
mod query;
mod schema;
mod writer;

use std::path::Path;
use std::sync::mpsc::{SyncSender, channel, sync_channel};

use rusqlite::Connection;

pub use dto::{
    HostStat, LogPage, LogQuery, LogStats, LogStatsQuery, OutcomeStat, ReqRecord, SeriesPoint,
};
pub use outcome::Outcome;

use schema::{init_schema, set_db_mode};
use writer::{LogCmd, WriterState, handle_cmd, recv_result};

const CHANNEL_CAP: usize = 8192;
const DEFAULT_MAX_ROWS: u64 = 10_000;

/// Cloneable handle to the request-log writer thread.
#[derive(Clone)]
pub struct LogStore {
    tx: SyncSender<LogCmd>,
}

impl LogStore {
    /// Open (or create) the SQLite database at `path` and start the writer thread.
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

    /// Return one filtered page of recorded requests.
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

    /// Return aggregate statistics over the persisted request history.
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

    /// Delete every recorded request; returns the number of removed rows.
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

    /// Enforce retention: drop rows older than `max_age_days`, then trim to `max_rows`.
    /// Returns `(deleted_by_age, deleted_by_rows)`.
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

#[cfg(test)]
mod tests {
    use super::query::rustcache_now_ms;
    use super::schema::{index_exists, table_exists};
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

    fn rec(
        ts: u64,
        method: &str,
        url: &str,
        host: &str,
        status: u16,
        outcome: Outcome,
    ) -> ReqRecord {
        ReqRecord {
            ts,
            method: method.into(),
            url: url.into(),
            host: host.into(),
            status,
            outcome,
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
                store.enqueue(rec(
                    i,
                    "GET",
                    &format!("http://e/{i}"),
                    "e",
                    200,
                    Outcome::Hit,
                ));
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
                Outcome::Hit,
            ));
            store.enqueue(rec(
                2,
                "GET",
                "http://other.test/b",
                "other.test",
                200,
                Outcome::Miss,
            ));
            store.enqueue(rec(3, "GET", "http://x/pct%100", "x", 200, Outcome::Hit));
            store.enqueue(rec(
                4,
                "GET",
                "http://x/under_score",
                "x",
                200,
                Outcome::Hit,
            ));
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
            store.enqueue(rec(1, "GET", "http://a", "a", 200, Outcome::Hit));
            store.enqueue(rec(2, "POST", "http://b", "b", 404, Outcome::Miss));
            store.enqueue(rec(3, "GET", "http://c", "c", 500, Outcome::Error));
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
                    outcome: Some(Outcome::Miss),
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
                    outcome: Some(Outcome::Hit),
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
            store.enqueue(rec(100, "GET", "http://a", "a", 200, Outcome::Hit));
            store.enqueue(rec(200, "GET", "http://b", "b", 200, Outcome::Hit));
            store.enqueue(rec(300, "GET", "http://c", "c", 200, Outcome::Hit));
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
                store.enqueue(rec(
                    i,
                    "GET",
                    &format!("http://e/{i}"),
                    "e",
                    200,
                    Outcome::Hit,
                ));
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
            store.enqueue(rec(old, "GET", "http://old", "old", 200, Outcome::Hit));
            store.enqueue(rec(now, "GET", "http://new", "new", 200, Outcome::Hit));
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
                    Outcome::Hit,
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
                store.enqueue(rec(i, "GET", "http://e", "e", 200, Outcome::Hit));
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
                store.enqueue(rec(i, "GET", "http://e", "e", 200, Outcome::Hit));
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
        outcome: Outcome,
        duration_ms: u64,
        resp_bytes: u64,
    ) -> ReqRecord {
        ReqRecord {
            ts,
            method: "GET".into(),
            url: format!("http://{host}/"),
            host: host.into(),
            status: 200,
            outcome,
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
            assert_eq!(s.by_outcome.len(), 9);
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
            store.enqueue(rec_full(1000, "a", Outcome::Hit, 10, 100));
            store.enqueue(rec_full(2000, "a", Outcome::Hit, 20, 200));
            store.enqueue(rec_full(3000, "a", Outcome::HitRevalidated, 30, 300));
            store.enqueue(rec_full(4000, "a", Outcome::Miss, 40, 400));
            store.enqueue(rec_full(5000, "a", Outcome::Revalidated, 50, 500));
            store.enqueue(rec_full(6000, "a", Outcome::Error, 60, 600));
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
            store.enqueue(rec_full(1000, "a", Outcome::Hit, 1, 10));
            store.enqueue(rec_full(2000, "a", Outcome::Miss, 1, 10));
            store.flush().await.expect("flush");

            let s = store.stats(LogStatsQuery::default()).await.expect("stats");
            assert_eq!(s.by_outcome.len(), 9);
            assert_eq!(s.by_outcome[0].outcome, Outcome::Hit);
            assert_eq!(s.by_outcome[0].count, 1);
            assert_eq!(s.by_outcome[4].outcome, Outcome::Miss);
            assert_eq!(s.by_outcome[4].count, 1);
            assert_eq!(s.by_outcome[1].count, 0);
            assert_eq!(s.by_outcome[8].outcome, Outcome::RejectCmd);
            assert_eq!(s.by_outcome[8].count, 0);
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
                    store.enqueue(rec_full(1000, &host, Outcome::Hit, 1, 5));
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
            store.enqueue(rec_full(0, "a", Outcome::Hit, 1, 10));
            store.enqueue(rec_full(600_000, "a", Outcome::Miss, 1, 10));
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
            store.enqueue(rec_full(1000, "a", Outcome::Hit, 1, 10));
            store.enqueue(rec_full(2000, "b", Outcome::Miss, 1, 10));
            store.enqueue(rec_full(3000, "c", Outcome::Error, 1, 10));
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

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

enum LogCmd {
    Insert(ReqRecord),
    Query(LogQuery, std::sync::mpsc::Sender<anyhow::Result<LogPage>>),
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
}

//! Writer-thread machinery: command channel messages, single-threaded SQLite writes, reply helpers.

use rusqlite::{Connection, params};

use super::analytics::stats;
use super::dto::{LogPage, LogQuery, LogStats, LogStatsQuery, ReqRecord};
use super::query::{cleanup, clear, query, trim_rows};

const INSERT_TRIM_EVERY: u64 = 256;

/// Message sent from API/request threads to the dedicated log writer thread.
pub(super) enum LogCmd {
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

/// Mutable state owned by the writer thread.
pub(super) struct WriterState {
    pub(super) conn: Connection,
    pub(super) inserts_since_trim: u64,
    pub(super) max_rows: u64,
    pub(super) cleanup_runs: u64,
}

/// Run one writer command against the open connection.
pub(super) fn handle_cmd(state: &mut WriterState, cmd: LogCmd) {
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
            rec.outcome.as_str(),
            rec.duration_ms as i64,
            rec.resp_bytes as i64,
        ],
    )?;
    Ok(())
}

/// Receive the single writer reply, mapping a dropped channel to an error.
pub(super) fn recv_result<T>(
    rx: std::sync::mpsc::Receiver<anyhow::Result<T>>,
) -> anyhow::Result<T> {
    match rx.recv() {
        Ok(r) => r,
        Err(_) => anyhow::bail!("log writer dropped reply"),
    }
}

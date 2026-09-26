//! SQL filter building and request-log row operations: page query, clear, trim, cleanup.

use rusqlite::{Connection, params};

use super::dto::{LogPage, LogQuery, ReqRecord};
use super::outcome::Outcome;

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

/// Rendered `WHERE` clause plus its bind parameters.
pub(super) struct FilterSql {
    pub(super) where_sql: String,
    pub(super) params: Vec<rusqlite::types::Value>,
}

pub(super) fn build_filter(q: &LogQuery) -> FilterSql {
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
    if let Some(o) = q.outcome {
        where_sql.push_str(" AND outcome = ?");
        params.push(Value::Text(o.as_str().to_string()));
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

pub(super) fn query(conn: &Connection, q: &LogQuery) -> anyhow::Result<LogPage> {
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
            outcome: Outcome::from_db_lossy(&r.get::<_, String>(5)?),
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

/// Delete every recorded request, returning the number of removed rows.
pub(super) fn clear(conn: &Connection) -> anyhow::Result<u64> {
    let n = conn.execute("DELETE FROM requests", [])?;
    Ok(n as u64)
}

/// Keep only the newest `max_rows` rows and delete the rest.
pub(super) fn trim_rows(conn: &Connection, max_rows: u64) -> anyhow::Result<u64> {
    let max_rows = max_rows.max(1) as i64;
    let n = conn.execute(
        "DELETE FROM requests WHERE id NOT IN (
            SELECT id FROM requests ORDER BY ts DESC, id DESC LIMIT ?1
        )",
        params![max_rows],
    )?;
    Ok(n as u64)
}

/// Drop rows older than `max_age_days`, then trim to `max_rows`.
/// Returns `(deleted_by_age, deleted_by_rows)`.
pub(super) fn cleanup(
    conn: &Connection,
    max_rows: u64,
    max_age_days: u64,
) -> anyhow::Result<(u64, u64)> {
    let now = rustcache_now_ms();
    let cutoff = now.saturating_sub(max_age_days.saturating_mul(86_400_000)) as i64;
    let by_age = conn.execute("DELETE FROM requests WHERE ts < ?1", params![cutoff])? as u64;
    let by_rows = trim_rows(conn, max_rows)?;
    Ok((by_age, by_rows))
}

pub(super) fn rustcache_now_ms() -> u64 {
    crate::cache::meta::now_ms()
}

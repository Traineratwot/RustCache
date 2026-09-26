//! Aggregate statistics over the request log: totals, per-outcome, top hosts, and time series.

use rusqlite::Connection;

use super::dto::{HostStat, LogQuery, LogStats, LogStatsQuery, OutcomeStat, SeriesPoint};
use super::outcome::Outcome;
use super::query::build_filter;

/// Canonical outcome labels, always present in `by_outcome` (zero-filled).
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

pub(super) fn stats(conn: &Connection, q: &LogStatsQuery) -> anyhow::Result<LogStats> {
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
                COALESCE(SUM(CASE WHEN outcome IN ('HIT','HIT_REVALIDATED','HIT_STALE') THEN 1 ELSE 0 END),0), \
                COALESCE(SUM(CASE WHEN outcome IN ('MISS','REVALIDATED') THEN 1 ELSE 0 END),0), \
                COALESCE(SUM(CASE WHEN outcome IN ('HIT','HIT_REVALIDATED','HIT_STALE') THEN resp_bytes ELSE 0 END),0) \
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

    let mut by_outcome: Vec<OutcomeStat> = Vec::with_capacity(Outcome::ALL.len() + 4);
    for canon in Outcome::ALL {
        let found = raw_outcomes.iter().find(|(o, ..)| o == canon.as_str());
        match found {
            Some((outcome, count, bytes, avg)) => by_outcome.push(OutcomeStat {
                outcome: Outcome::from_db_lossy(outcome),
                count: *count,
                bytes: *bytes,
                avg_duration_ms: *avg,
            }),
            None => by_outcome.push(OutcomeStat {
                outcome: canon,
                count: 0,
                bytes: 0,
                avg_duration_ms: 0.0,
            }),
        }
    }
    let mut extras: Vec<(String, u64, u64, f64)> = raw_outcomes
        .into_iter()
        .filter(|(o, ..)| !Outcome::ALL.iter().any(|c| c.as_str() == o.as_str()))
        .collect();
    extras.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    for (outcome, count, bytes, avg) in extras {
        by_outcome.push(OutcomeStat {
            outcome: Outcome::from_db_lossy(&outcome),
            count,
            bytes,
            avg_duration_ms: avg,
        });
    }

    // Q3: top_hosts
    let top_hosts_sql = format!(
        "SELECT host, COUNT(*), COALESCE(SUM(resp_bytes),0), \
                COALESCE(SUM(CASE WHEN outcome IN ('HIT','HIT_REVALIDATED','HIT_STALE') THEN 1 ELSE 0 END),0) \
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
                    COALESCE(SUM(CASE WHEN outcome IN ('HIT','HIT_REVALIDATED','HIT_STALE') THEN 1 ELSE 0 END),0), \
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

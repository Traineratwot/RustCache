//! HTTP cache-policy evaluation: Cache-Control, Expires, ETag, Vary, status.

use std::time::Duration;

use crate::cache::meta::CacheMeta;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheDecision {
    StoreAndCache,
    StoreButRevalidate,
    Bypass,
    NoStore,
}

#[derive(Debug, Clone, Default)]
pub struct CachePolicy {
    pub max_age: Option<u64>,
    pub s_maxage: Option<u64>,
    pub no_cache: bool,
    pub no_store: bool,
    pub private: bool,
    pub public: bool,
    pub must_revalidate: bool,
    pub immutable: bool,
    pub vary: Option<String>,
    pub expires_at: Option<u64>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

impl CachePolicy {
    pub fn from_headers(status: u16, headers: &[(String, String)]) -> Self {
        let mut p = CachePolicy::default();
        let mut date_ms: Option<u64> = None;
        let mut max_age_header: Option<u64> = None;

        for (name, value) in headers {
            let n = name.to_ascii_lowercase();
            match n.as_str() {
                "cache-control" => parse_cache_control(value, &mut p, &mut max_age_header),
                "expires" => p.expires_at = parse_http_date(value),
                "date" => date_ms = parse_http_date(value),
                "etag" => p.etag = Some(value.clone()),
                "last-modified" => p.last_modified = Some(value.clone()),
                "vary" => {
                    let v = value.trim();
                    if !v.is_empty() && v != "*" {
                        p.vary = Some(v.to_ascii_lowercase());
                    }
                }
                _ => {}
            }
        }

        // Expires is relative to Date when both present; if only Expires, use absolute.
        if let (Some(d), Some(exp)) = (date_ms, p.expires_at) {
            // parse_http_date stored absolute millis; if Expires was parsed as absolute
            // we keep it. When both exist and Expires < Date, treat as already stale.
            if exp < d {
                p.expires_at = Some(0);
            }
        }

        if let Some(ma) = p.s_maxage.or(p.max_age).or(max_age_header) {
            let base = date_ms.unwrap_or_else(now_secs_ms);
            if !p.immutable {
                p.expires_at = Some(base + ma * 1000);
            }
        } else if p.immutable {
            p.expires_at = Some(now_secs_ms() + 365 * 24 * 3600 * 1000);
        }

        // Default heuristic freshness for 200 with Last-Modified (10% of age).
        if p.expires_at.is_none() && status == 200 && !p.no_store && !p.no_cache {
            if let Some(lm) = p.last_modified.clone() {
                if let Some(lm_ms) = parse_http_date(&lm) {
                    let age = now_secs_ms().saturating_sub(lm_ms);
                    p.expires_at = Some(now_secs_ms() + age / 10);
                }
            }
        }

        p
    }

    pub fn decide(&self, method_is_get_head: bool) -> CacheDecision {
        if !method_is_get_head || self.no_store || self.private {
            return CacheDecision::NoStore;
        }
        if self.no_cache || self.must_revalidate {
            return CacheDecision::StoreButRevalidate;
        }
        match self.expires_at {
            Some(_) => CacheDecision::StoreAndCache,
            None => CacheDecision::StoreButRevalidate,
        }
    }

    pub fn ttl(&self) -> Option<Duration> {
        let exp = self.expires_at?;
        let now = now_secs_ms();
        if exp <= now {
            return Some(Duration::from_secs(0));
        }
        Some(Duration::from_millis(exp - now))
    }

    pub fn to_meta(
        &self,
        key: &str,
        url: &str,
        status: u16,
        headers: Vec<(String, String)>,
    ) -> CacheMeta {
        CacheMeta {
            key: key.to_string(),
            url: url.to_string(),
            status,
            headers,
            stored_at: now_secs_ms(),
            last_access: now_secs_ms(),
            body_len: 0,
            etag: self.etag.clone(),
            last_modified: self.last_modified.clone(),
            expires_at: self.expires_at,
            cacheable: matches!(
                self.decide(true),
                CacheDecision::StoreAndCache | CacheDecision::StoreButRevalidate
            ),
        }
    }
}

fn parse_cache_control(value: &str, p: &mut CachePolicy, max_age_header: &mut Option<u64>) {
    for part in value.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (name, val) = match part.split_once('=') {
            Some((n, v)) => (
                n.trim().to_ascii_lowercase(),
                Some(v.trim().trim_matches('"')),
            ),
            None => (part.to_ascii_lowercase(), None),
        };
        match name.as_str() {
            "no-store" => p.no_store = true,
            "no-cache" => p.no_cache = true,
            "private" => p.private = true,
            "public" => p.public = true,
            "must-revalidate" => p.must_revalidate = true,
            "immutable" => p.immutable = true,
            "max-age" => {
                if let Some(v) = val.and_then(|v| v.parse::<u64>().ok()) {
                    p.max_age = Some(v);
                    *max_age_header = Some(v);
                }
            }
            "s-maxage" => {
                if let Some(v) = val.and_then(|v| v.parse::<u64>().ok()) {
                    p.s_maxage = Some(v);
                }
            }
            _ => {}
        }
    }
}

fn parse_http_date(v: &str) -> Option<u64> {
    // Accept IMF-fixdate, RFC 850, asctime — implement IMF-fixdate primarily.
    let v = v.trim();
    // Try time via manual parse of "Thu, 01 Jan 1970 00:00:00 GMT"
    if let Some(ms) = parse_imf_fixdate(v) {
        return Some(ms);
    }
    // Naive fallback: if caller passed unix seconds
    if let Ok(secs) = v.parse::<u64>() {
        return Some(secs * 1000);
    }
    None
}

fn parse_imf_fixdate(v: &str) -> Option<u64> {
    // e.g. Wed, 21 Oct 2015 07:28:00 GMT
    let v = v.trim();
    let v = v.strip_suffix(" GMT").or_else(|| v.strip_suffix(" UTC"))?;
    let (_dow, rest) = v.split_once(", ")?;
    let mut parts = rest.split_whitespace();
    let day: u32 = parts.next()?.parse().ok()?;
    let month = month_num(parts.next()?)?;
    let year: i64 = parts.next()?.parse().ok()?;
    let time = parts.next()?;
    let mut tp = time.split(':');
    let hour: u64 = tp.next()?.parse().ok()?;
    let min: u64 = tp.next()?.parse().ok()?;
    let sec: u64 = tp.next()?.parse().ok()?;
    let days = days_from_civil(year, month, day);
    let secs = (days as u64) * 86400 + hour * 3600 + min * 60 + sec;
    Some(secs * 1000)
}

fn month_num(m: &str) -> Option<u32> {
    Some(match m {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    })
}

/// Howard Hinnant's civil_from_days inverse.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = ((m + 9) % 12) as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn now_secs_ms() -> u64 {
    crate::cache::meta::now_ms()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn max_age_makes_fresh() {
        let p = CachePolicy::from_headers(200, &h(&[("cache-control", "max-age=60")]));
        assert_eq!(p.decide(true), CacheDecision::StoreAndCache);
        assert!(p.ttl().unwrap().as_secs() <= 60);
    }

    #[test]
    fn no_store_bypasses() {
        let p = CachePolicy::from_headers(200, &h(&[("cache-control", "no-store")]));
        assert_eq!(p.decide(true), CacheDecision::NoStore);
    }

    #[test]
    fn no_cache_stores_but_revalidates() {
        let p = CachePolicy::from_headers(200, &h(&[("cache-control", "no-cache")]));
        assert_eq!(p.decide(true), CacheDecision::StoreButRevalidate);
    }

    #[test]
    fn private_is_not_stored() {
        let p = CachePolicy::from_headers(200, &h(&[("cache-control", "private, max-age=60")]));
        assert_eq!(p.decide(true), CacheDecision::NoStore);
    }

    #[test]
    fn etag_captured() {
        let p = CachePolicy::from_headers(200, &h(&[("etag", "\"xyz\"")]));
        assert_eq!(p.etag.as_deref(), Some("\"xyz\""));
    }

    #[test]
    fn post_is_not_stored() {
        let p = CachePolicy::from_headers(200, &h(&[("cache-control", "max-age=60")]));
        assert_eq!(p.decide(false), CacheDecision::NoStore);
    }

    #[test]
    fn expires_header_makes_fresh() {
        // 2100-01-01 — far future IMF-fixdate
        let p = CachePolicy::from_headers(200, &h(&[("expires", "Fri, 01 Jan 2100 00:00:00 GMT")]));
        assert_eq!(p.decide(true), CacheDecision::StoreAndCache);
        assert!(p.ttl().is_some());
        assert!(p.ttl().unwrap().as_secs() > 0);
    }

    #[test]
    fn expired_expires_header_is_stale() {
        let p = CachePolicy::from_headers(200, &h(&[("expires", "Thu, 01 Jan 1970 00:00:00 GMT")]));
        assert_eq!(p.expires_at, Some(0));
        assert_eq!(p.ttl().unwrap().as_secs(), 0);
    }

    #[test]
    fn vary_star_is_ignored() {
        let p =
            CachePolicy::from_headers(200, &h(&[("vary", "*"), ("cache-control", "max-age=60")]));
        assert!(p.vary.is_none());
    }

    #[test]
    fn vary_header_captured_lowercase() {
        let p = CachePolicy::from_headers(
            200,
            &h(&[
                ("vary", "Accept-Encoding, User-Agent"),
                ("cache-control", "max-age=60"),
            ]),
        );
        assert_eq!(p.vary.as_deref(), Some("accept-encoding, user-agent"));
    }

    #[test]
    fn s_maxage_overrides_max_age() {
        let p =
            CachePolicy::from_headers(200, &h(&[("cache-control", "max-age=60, s-maxage=120")]));
        assert_eq!(p.s_maxage, Some(120));
        let ttl = p.ttl().unwrap().as_secs();
        assert!(ttl > 60 && ttl <= 120);
    }

    #[test]
    fn must_revalidate_stores_but_revalidates() {
        let p =
            CachePolicy::from_headers(200, &h(&[("cache-control", "max-age=60, must-revalidate")]));
        assert!(p.must_revalidate);
        assert_eq!(p.decide(true), CacheDecision::StoreButRevalidate);
    }

    #[test]
    fn immutable_extends_freshness() {
        let p = CachePolicy::from_headers(200, &h(&[("cache-control", "immutable")]));
        assert!(p.immutable);
        assert_eq!(p.decide(true), CacheDecision::StoreAndCache);
        assert!(p.ttl().unwrap().as_secs() > 3600);
    }
}

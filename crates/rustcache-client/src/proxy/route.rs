//! Routing decision: MITM / raw tunnel / DIRECT.

use crate::health::HealthMonitor;
use crate::proxy::http_parse::{host_only, matches_bypass};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// HTTPS via RustCache MITM (:3129) — caching + inspection.
    Mitm,
    /// Via RustCache HTTP proxy (:3128) — CONNECT is raw tunnel, plain HTTP is cached.
    RawTunnel,
    /// Straight to the origin.
    Direct,
}

pub struct RouteDecider<'a> {
    health: &'a HealthMonitor,
    bypass: &'a [String],
    upstream_ok: bool,
}

impl<'a> RouteDecider<'a> {
    pub fn new(health: &'a HealthMonitor, bypass: &'a [String], upstream_ok: bool) -> Self {
        Self {
            health,
            bypass,
            upstream_ok,
        }
    }

    /// CONNECT (HTTPS) routing.
    pub fn decide_connect(&self, target: &str) -> Route {
        let host = host_only(target);
        if matches_bypass(host, self.bypass) {
            return Route::Direct;
        }
        if !self.upstream_ok || !self.health.use_upstream() {
            return Route::Direct;
        }
        if self.health.use_mitm() {
            Route::Mitm
        } else {
            // HTTPS listener down — avoid blackhole; tunnel raw via :3128 or go direct.
            if self.health.use_http_upstream() {
                Route::RawTunnel
            } else {
                Route::Direct
            }
        }
    }

    /// Plain HTTP routing (`GET http://…`).
    ///
    /// Prefer the absolute-URI host when present — a Host header can be stale
    /// or a placeholder while the request-line carries the real target.
    pub fn decide_http(&self, target: &str, host: Option<&str>) -> Route {
        let from_target = crate::proxy::http_parse::host_from_target(target);
        let h = match from_target {
            Some(t) => host_only(&t).to_string(),
            None => host.map(|h| host_only(h).to_string()).unwrap_or_default(),
        };
        let h = h.as_str();
        if matches_bypass(h, self.bypass) {
            return Route::Direct;
        }
        if self.health.use_http_upstream() {
            Route::RawTunnel
        } else {
            Route::Direct
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ClientConfig;
    use crate::health::HealthMonitor;

    fn mon_up() -> HealthMonitor {
        HealthMonitor::new(&ClientConfig::default())
    }

    fn mon_down() -> HealthMonitor {
        let m = HealthMonitor::new(&ClientConfig::default());
        for _ in 0..5 {
            m.record_failure("down");
        }
        m
    }

    #[test]
    fn connect_goes_mitm_when_up() {
        let m = mon_up();
        let bypass = vec!["localhost".to_string()];
        let d = RouteDecider::new(&m, &bypass, true);
        assert_eq!(d.decide_connect("example.com:443"), Route::Mitm);
    }

    #[test]
    fn connect_direct_when_open() {
        let m = mon_down();
        let bypass = vec![];
        let d = RouteDecider::new(&m, &bypass, false);
        assert_eq!(d.decide_connect("example.com:443"), Route::Direct);
    }

    #[test]
    fn bypass_forces_direct() {
        let m = mon_up();
        let bypass = vec!["localhost".into(), "*.local".into()];
        let d = RouteDecider::new(&m, &bypass, true);
        assert_eq!(d.decide_connect("localhost:443"), Route::Direct);
        assert_eq!(d.decide_connect("printer.local:443"), Route::Direct);
    }

    #[test]
    fn http_upstream_when_closed() {
        let m = mon_up();
        let d = RouteDecider::new(&m, &[], true);
        assert_eq!(
            d.decide_http("http://example.com/", Some("example.com")),
            Route::RawTunnel
        );
    }
}

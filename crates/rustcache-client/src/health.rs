//! Health polling + circuit breaker for fail-open to DIRECT.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use parking_lot::RwLock;

use crate::api::{HealthReport, RustCacheApi};
use crate::config::ClientConfig;

/// Breaker state as u8 for lock-free reads on the accept path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BreakerState {
    /// Use RustCache.
    Closed = 0,
    /// Use DIRECT.
    Open = 1,
    /// One trial request allowed.
    HalfOpen = 2,
}

impl BreakerState {
    fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Closed,
            1 => Self::Open,
            _ => Self::HalfOpen,
        }
    }
}

/// Shared health snapshot used by the proxy router.
#[derive(Debug, Clone)]
pub struct HealthSnapshot {
    pub state: BreakerState,
    pub http_running: bool,
    pub https_running: bool,
    pub socks_running: bool,
    pub last_ok: Option<Instant>,
    pub last_error: Option<String>,
    pub consecutive_failures: u32,
}

impl Default for HealthSnapshot {
    fn default() -> Self {
        Self {
            state: BreakerState::Closed,
            http_running: true,
            https_running: true,
            socks_running: true,
            last_ok: None,
            last_error: None,
            consecutive_failures: 0,
        }
    }
}

pub struct HealthMonitor {
    api: RustCacheApi,
    fail_open: bool,
    fail_threshold: u32,
    open_ms: u64,
    interval: Duration,
    state: AtomicU8,
    snap: RwLock<HealthSnapshot>,
    opened_at: RwLock<Option<Instant>>,
}

impl HealthMonitor {
    pub fn new(cfg: &ClientConfig) -> Self {
        Self {
            api: RustCacheApi::new(&cfg.rustcache_api),
            fail_open: cfg.fail_open,
            fail_threshold: cfg.breaker_failures,
            open_ms: cfg.breaker_open_ms,
            interval: Duration::from_millis(cfg.health_interval_ms.max(100)),
            state: AtomicU8::new(BreakerState::Closed as u8),
            snap: RwLock::new(HealthSnapshot::default()),
            opened_at: RwLock::new(None),
        }
    }

    pub fn snapshot(&self) -> HealthSnapshot {
        self.snap.read().clone()
    }

    pub fn state(&self) -> BreakerState {
        BreakerState::from_u8(self.state.load(Ordering::Acquire))
    }

    /// True when new traffic should go to RustCache (not DIRECT).
    pub fn use_upstream(&self) -> bool {
        match self.state() {
            BreakerState::Closed => true,
            BreakerState::HalfOpen => true,
            BreakerState::Open => false,
        }
    }

    /// HTTPS CONNECT should use MITM only when circuit closed and HTTPS listener is up.
    pub fn use_mitm(&self) -> bool {
        let s = self.snap.read();
        s.state == BreakerState::Closed && s.https_running
    }

    pub fn use_http_upstream(&self) -> bool {
        let s = self.snap.read();
        (s.state == BreakerState::Closed || s.state == BreakerState::HalfOpen) && s.http_running
    }

    fn set_state(&self, st: BreakerState) {
        self.state.store(st as u8, Ordering::Release);
        self.snap.write().state = st;
    }

    /// Record a successful upstream use (closes half-open).
    pub fn record_success(&self) {
        let mut s = self.snap.write();
        s.consecutive_failures = 0;
        s.last_ok = Some(Instant::now());
        s.last_error = None;
        s.state = BreakerState::Closed;
        drop(s);
        self.state
            .store(BreakerState::Closed as u8, Ordering::Release);
        *self.opened_at.write() = None;
    }

    /// Record a failure; may open the breaker.
    pub fn record_failure(&self, err: impl Into<String>) {
        if !self.fail_open {
            // fail-closed: never open, just note the error
            self.snap.write().last_error = Some(err.into());
            return;
        }
        let mut s = self.snap.write();
        s.consecutive_failures = s.consecutive_failures.saturating_add(1);
        s.last_error = Some(err.into());
        let fails = s.consecutive_failures;
        drop(s);

        if self.state() == BreakerState::HalfOpen || fails >= self.fail_threshold {
            self.set_state(BreakerState::Open);
            *self.opened_at.write() = Some(Instant::now());
        }
    }

    /// Background poller. Runs until cancelled.
    pub async fn run(self: Arc<Self>) {
        let mut tick = tokio::time::interval(self.interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            self.probe_once().await;
        }
    }

    /// Single probe (also used by tests and `doctor`).
    pub async fn probe_once(&self) {
        // Transition Open → HalfOpen after open_ms.
        if self.state() == BreakerState::Open {
            let opened = *self.opened_at.read();
            if let Some(t0) = opened {
                if t0.elapsed() >= Duration::from_millis(self.open_ms) {
                    self.set_state(BreakerState::HalfOpen);
                }
            }
        }

        match self.api.health().await {
            Ok(rep) => self.apply_health(&rep),
            Err(e) => {
                // API down — try TCP probes as a weaker signal.
                let host = self.api_host();
                let http_up = crate::api::tcp_alive(&host, 3128, Duration::from_millis(300)).await;
                let https_up = crate::api::tcp_alive(&host, 3129, Duration::from_millis(300)).await;
                if http_up || https_up {
                    // Ports accept — treat as degraded success (API may be on another bind).
                    let mut s = self.snap.write();
                    s.http_running = http_up;
                    s.https_running = https_up;
                    s.socks_running = false;
                    s.consecutive_failures = 0;
                    s.last_ok = Some(Instant::now());
                    s.last_error = None;
                    s.state = BreakerState::Closed;
                    drop(s);
                    self.state
                        .store(BreakerState::Closed as u8, Ordering::Release);
                } else {
                    self.record_failure(format!("health: {e}"));
                }
            }
        }
    }

    fn apply_health(&self, rep: &HealthReport) {
        let healthy = rep.ok && (rep.http_running || rep.https_running);
        if healthy {
            let mut s = self.snap.write();
            s.http_running = rep.http_running;
            s.https_running = rep.https_running;
            s.socks_running = rep.socks_running;
            s.consecutive_failures = 0;
            s.last_ok = Some(Instant::now());
            s.last_error = None;
            s.state = BreakerState::Closed;
            drop(s);
            self.state
                .store(BreakerState::Closed as u8, Ordering::Release);
            *self.opened_at.write() = None;
        } else {
            let mut s = self.snap.write();
            s.http_running = rep.http_running;
            s.https_running = rep.https_running;
            s.socks_running = rep.socks_running;
            drop(s);
            self.record_failure("health: not ok");
        }
    }

    fn api_host(&self) -> String {
        self.api
            .base()
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .split(':')
            .next()
            .unwrap_or("127.0.0.1")
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon() -> HealthMonitor {
        let cfg = ClientConfig {
            breaker_failures: 3,
            breaker_open_ms: 50,
            ..Default::default()
        };
        HealthMonitor::new(&cfg)
    }

    #[test]
    fn opens_after_threshold() {
        let m = mon();
        assert!(m.use_upstream());
        m.record_failure("a");
        m.record_failure("b");
        assert_eq!(m.state(), BreakerState::Closed);
        m.record_failure("c");
        assert_eq!(m.state(), BreakerState::Open);
        assert!(!m.use_upstream());
    }

    #[test]
    fn half_open_then_close() {
        let m = mon();
        m.record_failure("x");
        m.record_failure("y");
        m.record_failure("z");
        assert_eq!(m.state(), BreakerState::Open);
        std::thread::sleep(Duration::from_millis(60));
        // force half-open transition via probe path logic
        if let Some(t0) = *m.opened_at.read() {
            if t0.elapsed() >= Duration::from_millis(m.open_ms) {
                m.set_state(BreakerState::HalfOpen);
            }
        }
        assert_eq!(m.state(), BreakerState::HalfOpen);
        m.record_success();
        assert_eq!(m.state(), BreakerState::Closed);
    }

    #[test]
    fn fail_closed_never_opens() {
        let cfg = ClientConfig {
            fail_open: false,
            ..Default::default()
        };
        let m = HealthMonitor::new(&cfg);
        for _ in 0..10 {
            m.record_failure("e");
        }
        assert_eq!(m.state(), BreakerState::Closed);
    }
}

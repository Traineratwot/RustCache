//! Atomic counters for proxy runtime stats.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Default)]
pub struct Metrics {
    pub hits: AtomicU64,
    pub misses: AtomicU64,
    pub revalidations: AtomicU64,
    pub bypasses: AtomicU64,
    pub bytes_served: AtomicU64,
    pub bytes_saved: AtomicU64,
    pub errors: AtomicU64,
    pub tunnels: AtomicU64,
}

pub type SharedMetrics = Arc<Metrics>;

impl Metrics {
    pub fn shared() -> SharedMetrics {
        Arc::new(Self::default())
    }

    pub fn add_hit(&self, saved: u64) {
        self.hits.fetch_add(1, Ordering::Relaxed);
        self.bytes_saved.fetch_add(saved, Ordering::Relaxed);
    }

    pub fn add_miss(&self) {
        self.misses.fetch_add(1, Ordering::Relaxed);
    }

    pub fn add_revalidation(&self) {
        self.revalidations.fetch_add(1, Ordering::Relaxed);
    }

    pub fn add_bypass(&self) {
        self.bypasses.fetch_add(1, Ordering::Relaxed);
    }

    pub fn add_served(&self, bytes: u64) {
        self.bytes_served.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn add_error(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn add_tunnel(&self) {
        self.tunnels.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        let hits = self.hits.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);
        let total = hits + misses;
        MetricsSnapshot {
            hits,
            misses,
            revalidations: self.revalidations.load(Ordering::Relaxed),
            bypasses: self.bypasses.load(Ordering::Relaxed),
            bytes_served: self.bytes_served.load(Ordering::Relaxed),
            bytes_saved: self.bytes_saved.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            tunnels: self.tunnels.load(Ordering::Relaxed),
            hit_rate: if total == 0 {
                0.0
            } else {
                hits as f64 / total as f64
            },
            saved_mb: self.bytes_saved.load(Ordering::Relaxed) as f64 / (1024.0 * 1024.0),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MetricsSnapshot {
    pub hits: u64,
    pub misses: u64,
    pub revalidations: u64,
    pub bypasses: u64,
    pub bytes_served: u64,
    pub bytes_saved: u64,
    pub errors: u64,
    pub tunnels: u64,
    pub hit_rate: f64,
    pub saved_mb: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_and_hit_rate() {
        let m = Metrics::default();
        m.add_hit(1024);
        m.add_hit(2048);
        m.add_miss();
        let s = m.snapshot();
        assert_eq!(s.hits, 2);
        assert_eq!(s.misses, 1);
        assert_eq!(s.bytes_saved, 3072);
        assert!((s.hit_rate - 2.0 / 3.0).abs() < 1e-9);
    }
}

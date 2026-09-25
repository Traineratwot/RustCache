//! Fixed-capacity ring of recent request records.

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

pub const RING_CAPACITY: usize = 100;

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

pub struct ReqRing {
    inner: RwLock<VecDeque<ReqRecord>>,
    capacity: usize,
}

impl Default for ReqRing {
    fn default() -> Self {
        Self::new(RING_CAPACITY)
    }
}

impl ReqRing {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: RwLock::new(VecDeque::with_capacity(capacity)),
            capacity: capacity.max(1),
        }
    }

    pub fn push(&self, rec: ReqRecord) {
        let mut q = self.inner.write();
        if q.len() == self.capacity {
            q.pop_front();
        }
        q.push_back(rec);
    }

    /// Newest-last snapshot, optionally truncated to `limit` newest records.
    pub fn snapshot(&self, limit: usize) -> Vec<ReqRecord> {
        let q = self.inner.read();
        let n = q.len().min(limit.max(1));
        q.iter().rev().take(n).cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.inner.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(i: u64) -> ReqRecord {
        ReqRecord {
            ts: i,
            method: "GET".into(),
            url: format!("http://example.com/{i}"),
            host: "example.com".into(),
            status: 200,
            outcome: "HIT".into(),
            duration_ms: i,
            resp_bytes: 10,
        }
    }

    #[test]
    fn ring_caps_and_orders_newest_first_in_snapshot() {
        let r = ReqRing::new(3);
        for i in 0..5 {
            r.push(rec(i));
        }
        assert_eq!(r.len(), 3);
        let s = r.snapshot(10);
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].ts, 4);
        assert_eq!(s[1].ts, 3);
        assert_eq!(s[2].ts, 2);
    }
}

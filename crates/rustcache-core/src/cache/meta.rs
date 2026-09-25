//! Cache entry metadata stored next to the body.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheMeta {
    pub key: String,
    pub url: String,
    pub status: u16,
    pub headers: Vec<(String, String)>,
    /// Unix millis when stored.
    pub stored_at: u64,
    /// Unix millis of last access (for LRU).
    pub last_access: u64,
    pub body_len: u64,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    /// Absolute expiry (unix millis). `None` = treat as immediately stale (revalidate).
    pub expires_at: Option<u64>,
    /// True if the response is cacheable under our policy.
    pub cacheable: bool,
}

impl CacheMeta {
    pub fn is_fresh(&self, now_ms: u64) -> bool {
        if !self.cacheable {
            return false;
        }
        match self.expires_at {
            Some(exp) => now_ms < exp,
            None => false,
        }
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

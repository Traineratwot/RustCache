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

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(cacheable: bool, expires_at: Option<u64>) -> CacheMeta {
        CacheMeta {
            key: "k".into(),
            url: "u".into(),
            status: 200,
            headers: vec![],
            stored_at: 1000,
            last_access: 1000,
            body_len: 0,
            etag: None,
            last_modified: None,
            expires_at,
            cacheable,
        }
    }

    #[test]
    fn fresh_when_cacheable_and_before_expiry() {
        assert!(meta(true, Some(2000)).is_fresh(1500));
        assert!(!meta(true, Some(2000)).is_fresh(2000));
        assert!(!meta(true, Some(2000)).is_fresh(2500));
    }

    #[test]
    fn stale_without_expiry_or_when_not_cacheable() {
        assert!(!meta(true, None).is_fresh(1500));
        assert!(!meta(false, Some(2000)).is_fresh(1500));
    }
}

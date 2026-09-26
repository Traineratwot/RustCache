//! In-memory cache (moka) with body-len weigher. Disk remains source of truth.
//!
//! Freshness at read time is governed by [`CacheMeta::is_fresh`] (`expires_at`).
//! The optional per-entry `ttl` is only an upper bound on how long the entry is
//! held in memory; it never makes a stale entry look fresh.

use std::sync::Arc;
use std::time::Duration;

use moka::future::Cache as Moka;
use moka::Expiry;

use super::meta::CacheMeta;
use crate::Result;

/// A cached HTTP response body plus the metadata describing it.
#[derive(Clone)]
pub struct CachedEntry {
    pub meta: CacheMeta,
    pub body: Arc<Vec<u8>>,
}

/// Value stored in moka: the entry plus its optional memory TTL.
#[derive(Clone)]
struct MemValue {
    entry: CachedEntry,
    ttl: Option<Duration>,
}

/// Per-entry expiry so `insert(..., ttl)` is honored by moka.
///
/// Returning `None` leaves the entry under the cache-level `time_to_idle`
/// policy only.
struct MemExpiry;

impl Expiry<String, MemValue> for MemExpiry {
    fn expire_after_create(
        &self,
        _key: &String,
        value: &MemValue,
        _created_at: std::time::Instant,
    ) -> Option<Duration> {
        value.ttl
    }

    fn expire_after_update(
        &self,
        _key: &String,
        value: &MemValue,
        _updated_at: std::time::Instant,
        _prev: Option<Duration>,
    ) -> Option<Duration> {
        value.ttl
    }
}

/// In-memory hot cache. Not a source of truth — bodies are also on disk.
pub struct MemCache {
    inner: Moka<String, MemValue>,
}

impl MemCache {
    pub fn new(max_bytes: u64) -> Self {
        let inner = Moka::builder()
            .weigher(|_, v: &MemValue| (v.entry.meta.body_len as u32).max(1))
            .max_capacity(max_bytes.max(1))
            .time_to_idle(Duration::from_secs(3600))
            .expire_after(MemExpiry)
            .build();
        Self { inner }
    }

    /// Look up a cached entry by cache key. Returns `None` if absent or evicted.
    pub async fn get(&self, key: &str) -> Option<CachedEntry> {
        self.inner.get(key).await.map(|v| v.entry)
    }

    /// Insert or replace an entry.
    ///
    /// `ttl` is an optional upper bound on memory residency. Freshness for
    /// serving is still decided by `entry.meta.expires_at` at read time.
    pub async fn insert(&self, key: &str, entry: CachedEntry, ttl: Option<Duration>) {
        self.inner
            .insert(key.to_string(), MemValue { entry, ttl })
            .await;
    }

    pub async fn invalidate(&self, key: &str) {
        self.inner.invalidate(key).await;
    }

    pub async fn invalidate_all(&self) {
        self.inner.invalidate_all();
    }

    pub fn entry_count(&self) -> u64 {
        self.inner.entry_count()
    }

    /// Flush pending moka maintenance so tests can observe evictions.
    pub async fn sync(&self) -> Result<()> {
        self.inner.run_pending_tasks().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::meta::now_ms;

    fn entry(body: &[u8]) -> CachedEntry {
        CachedEntry {
            meta: CacheMeta {
                key: "k".into(),
                url: "u".into(),
                status: 200,
                headers: vec![],
                stored_at: now_ms(),
                last_access: now_ms(),
                body_len: body.len() as u64,
                etag: None,
                last_modified: None,
                expires_at: Some(now_ms() + 60_000),
                cacheable: true,
            },
            body: Arc::new(body.to_vec()),
        }
    }

    #[tokio::test]
    async fn insert_get_invalidate() {
        let mem = MemCache::new(1024);
        mem.insert("k", entry(b"abc"), None).await;
        assert!(mem.get("k").await.is_some());
        mem.invalidate("k").await;
        assert!(mem.get("k").await.is_none());
    }

    #[tokio::test]
    async fn weigher_evicts_under_capacity() {
        // capacity is in weighted body-bytes; insert more than max
        let mem = MemCache::new(10);
        mem.insert("a", entry(&vec![0u8; 8]), None).await;
        mem.insert("b", entry(&vec![0u8; 8]), None).await;
        mem.sync().await.unwrap();
        assert!(mem.entry_count() <= 1);
    }

    #[tokio::test]
    async fn invalidate_all_clears() {
        let mem = MemCache::new(1024);
        mem.insert("a", entry(b"1"), None).await;
        mem.insert("b", entry(b"2"), None).await;
        mem.invalidate_all().await;
        mem.sync().await.unwrap();
        assert_eq!(mem.entry_count(), 0);
    }

    #[tokio::test]
    async fn ttl_expires_entry_from_memory() {
        let mem = MemCache::new(1024);
        mem.insert("k", entry(b"abc"), Some(Duration::from_millis(50)))
            .await;
        assert!(mem.get("k").await.is_some());
        tokio::time::sleep(Duration::from_millis(80)).await;
        mem.sync().await.unwrap();
        assert!(mem.get("k").await.is_none());
    }

    #[tokio::test]
    async fn none_ttl_keeps_entry_alive() {
        let mem = MemCache::new(1024);
        mem.insert("k", entry(b"abc"), None).await;
        tokio::time::sleep(Duration::from_millis(30)).await;
        mem.sync().await.unwrap();
        assert!(mem.get("k").await.is_some());
    }
}

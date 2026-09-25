//! In-memory cache (moka) with body-len weigher. Disk remains source of truth.

use std::sync::Arc;
use std::time::Duration;

use moka::future::Cache as Moka;

use super::meta::CacheMeta;
use crate::Result;

#[derive(Clone)]
pub struct CachedEntry {
    pub meta: CacheMeta,
    pub body: Arc<Vec<u8>>,
}

pub struct MemCache {
    inner: Moka<String, CachedEntry>,
}

impl MemCache {
    pub fn new(max_bytes: u64) -> Self {
        let inner = Moka::builder()
            .weigher(|_, v: &CachedEntry| (v.meta.body_len as u32).max(1))
            .max_capacity(max_bytes.max(1))
            .time_to_idle(Duration::from_secs(3600))
            .build();
        Self { inner }
    }

    pub async fn get(&self, key: &str) -> Option<CachedEntry> {
        self.inner.get(key).await
    }

    pub async fn insert(&self, key: &str, entry: CachedEntry, ttl: Option<Duration>) {
        let _ = ttl;
        self.inner.insert(key.to_string(), entry).await;
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
}

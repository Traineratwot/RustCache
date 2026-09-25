//! LRU eviction by `last_access` when disk usage exceeds `max_bytes`.

use super::disk::DiskCache;
use crate::Result;

/// Evict least-recently-accessed entries until `bytes <= max_bytes`.
/// Returns number of entries removed.
pub async fn evict_lru(disk: &DiskCache, max_bytes: u64) -> Result<u64> {
    let (mut total, _entries) = disk.usage().await?;
    if total <= max_bytes {
        return Ok(0);
    }
    let mut items = disk.list_entries().await?;
    // oldest access first
    items.sort_by_key(|(_, last_access, _)| *last_access);
    let mut removed = 0u64;
    for (key, _, body_len) in items {
        if total <= max_bytes {
            break;
        }
        if disk.remove(&key).await? {
            total = total.saturating_sub(body_len);
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::disk::DiskCache;
    use crate::cache::key::cache_key;
    use crate::cache::meta::{now_ms, CacheMeta};

    #[tokio::test]
    async fn evicts_oldest_when_over_cap() {
        let dir = std::env::temp_dir().join(format!("rc-evict-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let disk = DiskCache::open(&dir).unwrap();
        for i in 0..3 {
            let url = format!("http://example.com/{i}");
            let key = cache_key(&url).as_str().to_string();
            let mut meta = CacheMeta {
                key: key.clone(),
                url: url.clone(),
                status: 200,
                headers: vec![],
                stored_at: 0,
                last_access: now_ms() - (3 - i) * 10_000,
                body_len: 100,
                etag: None,
                last_modified: None,
                expires_at: Some(now_ms() + 60_000),
                cacheable: true,
            };
            disk.store(meta.clone(), &vec![0u8; 100]).await.unwrap();
            // store overwrites last_access — rewrite meta to keep LRU order
            meta.last_access = now_ms() - (3 - i) * 10_000;
            let path_json = serde_json::to_vec(&meta).unwrap();
            let (a, b) = key.split_at(2);
            let b = &b[..2];
            let p = dir
                .join("index")
                .join(a)
                .join(b)
                .join(format!("{key}.json"));
            tokio::fs::write(&p, path_json).await.unwrap();
        }
        let removed = evict_lru(&disk, 150).await.unwrap();
        assert!(removed >= 1);
        let (total, _) = disk.usage().await.unwrap();
        assert!(total <= 150);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

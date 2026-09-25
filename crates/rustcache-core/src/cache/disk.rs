//! Disk cache: index/ + objects/ + tmp/ with atomic renames.

use std::path::{Path, PathBuf};

use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use super::meta::{now_ms, CacheMeta};
use crate::{Error, Result};

pub struct DiskCache {
    root: PathBuf,
    write_lock: Mutex<()>,
}

impl DiskCache {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(root.join("index"))?;
        std::fs::create_dir_all(root.join("objects"))?;
        std::fs::create_dir_all(root.join("tmp"))?;
        Ok(Self {
            root,
            write_lock: Mutex::new(()),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn index_path(&self, key: &str) -> PathBuf {
        let (a, b) = fanout(key);
        self.root
            .join("index")
            .join(a)
            .join(b)
            .join(format!("{key}.json"))
    }

    fn object_path(&self, key: &str) -> PathBuf {
        let (a, b) = fanout(key);
        self.root
            .join("objects")
            .join(a)
            .join(b)
            .join(format!("{key}.body"))
    }

    /// Load metadata if present.
    pub async fn load_meta(&self, key: &str) -> Result<Option<CacheMeta>> {
        if !is_hex_key(key) {
            return Ok(None);
        }
        let path = self.index_path(key);
        let bytes = match tokio::fs::read(&path).await {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let meta: CacheMeta = serde_json::from_slice(&bytes)?;
        Ok(Some(meta))
    }

    /// Load body bytes if present.
    pub async fn load_body(&self, key: &str) -> Result<Option<Vec<u8>>> {
        if !is_hex_key(key) {
            return Ok(None);
        }
        let path = self.object_path(key);
        match tokio::fs::read(&path).await {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Store meta + body atomically (tmp write → rename). Updates `last_access`.
    pub async fn store(&self, mut meta: CacheMeta, body: &[u8]) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let key = meta.key.clone();
        if !is_hex_key(&key) {
            return Err(Error::Cache(format!("non-hex cache key rejected: {key}")));
        }
        meta.body_len = body.len() as u64;
        let now = now_ms();
        meta.stored_at = now;
        meta.last_access = now;

        let index = self.index_path(&key);
        let object = self.object_path(&key);
        if let Some(parent) = index.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        if let Some(parent) = object.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let tmp_index = self.root.join("tmp").join(format!("{key}.json.tmp"));
        let tmp_object = self.root.join("tmp").join(format!("{key}.body.tmp"));

        write_atomic(&tmp_index, &index, &serde_json::to_vec(&meta)?).await?;
        write_atomic(&tmp_object, &object, body).await?;
        Ok(())
    }

    /// Touch `last_access` for LRU (best-effort).
    pub async fn touch(&self, key: &str) -> Result<()> {
        if !is_hex_key(key) {
            return Ok(());
        }
        let mut meta = match self.load_meta(key).await? {
            Some(m) => m,
            None => return Ok(()),
        };
        meta.last_access = now_ms();
        let path = self.index_path(key);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let tmp = self.root.join("tmp").join(format!("{key}.touch.tmp"));
        write_atomic(&tmp, &path, &serde_json::to_vec(&meta)?).await
    }

    pub async fn remove(&self, key: &str) -> Result<bool> {
        if !is_hex_key(key) {
            return Ok(false);
        }
        let _guard = self.write_lock.lock().await;
        let index = self.index_path(key);
        let object = self.object_path(key);
        let mut removed = false;
        if tokio::fs::remove_file(&index).await.is_ok() {
            removed = true;
        }
        if tokio::fs::remove_file(&object).await.is_ok() {
            removed = true;
        }
        Ok(removed)
    }

    /// Total size of objects in bytes and entry count.
    pub async fn usage(&self) -> Result<(u64, u64)> {
        let objects = self.root.join("objects");
        let mut total = 0u64;
        let mut entries = 0u64;
        let mut stack = vec![objects];
        while let Some(dir) = stack.pop() {
            let mut rd = match tokio::fs::read_dir(&dir).await {
                Ok(rd) => rd,
                Err(_) => continue,
            };
            while let Some(ent) = rd.next_entry().await? {
                let ft = ent.file_type().await?;
                if ft.is_dir() {
                    stack.push(ent.path());
                } else if ft.is_file() {
                    let meta = ent.metadata().await?;
                    total += meta.len();
                    entries += 1;
                }
            }
        }
        Ok((total, entries))
    }

    /// Remove every cache entry.
    pub async fn purge_all(&self) -> Result<u64> {
        let _guard = self.write_lock.lock().await;
        let mut purged = 0u64;
        for sub in ["index", "objects", "tmp"] {
            let dir = self.root.join(sub);
            purged += remove_tree_files(&dir).await?;
        }
        Ok(purged)
    }

    /// List `(key, last_access, body_len)` for eviction.
    pub async fn list_entries(&self) -> Result<Vec<(String, u64, u64)>> {
        let index_root = self.root.join("index");
        let mut out = Vec::new();
        let mut stack = vec![index_root];
        while let Some(dir) = stack.pop() {
            let mut rd = match tokio::fs::read_dir(&dir).await {
                Ok(rd) => rd,
                Err(_) => continue,
            };
            while let Some(ent) = rd.next_entry().await? {
                let path = ent.path();
                if ent.file_type().await?.is_dir() {
                    stack.push(path);
                } else if path.extension().map(|e| e == "json").unwrap_or(false) {
                    if let Ok(bytes) = tokio::fs::read(&path).await {
                        if let Ok(meta) = serde_json::from_slice::<CacheMeta>(&bytes) {
                            out.push((meta.key, meta.last_access, meta.body_len));
                        }
                    }
                }
            }
        }
        Ok(out)
    }
}

fn fanout(key: &str) -> (&str, &str) {
    let b = key.as_bytes();
    if b.len() >= 4 {
        (
            std::str::from_utf8(&b[0..2]).unwrap_or("00"),
            std::str::from_utf8(&b[2..4]).unwrap_or("00"),
        )
    } else {
        ("00", "00")
    }
}

fn is_hex_key(key: &str) -> bool {
    key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit())
}

async fn write_atomic(tmp: &Path, final_path: &Path, data: &[u8]) -> Result<()> {
    if let Some(parent) = tmp.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut f = tokio::fs::File::create(tmp).await?;
    f.write_all(data).await?;
    f.sync_all().await?;
    drop(f);
    if let Some(parent) = final_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::rename(tmp, final_path).await?;
    Ok(())
}

async fn remove_tree_files(dir: &Path) -> Result<u64> {
    let mut removed = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let mut rd = match tokio::fs::read_dir(&d).await {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        while let Some(ent) = rd.next_entry().await? {
            let path = ent.path();
            if ent.file_type().await?.is_dir() {
                stack.push(path);
            } else {
                if tokio::fs::remove_file(&path).await.is_ok() {
                    removed += 1;
                }
            }
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::key::cache_key;

    fn meta_for(url: &str) -> CacheMeta {
        let k = cache_key(url);
        CacheMeta {
            key: k.as_str().to_string(),
            url: url.to_string(),
            status: 200,
            headers: vec![("content-type".into(), "text/plain".into())],
            stored_at: 0,
            last_access: 0,
            body_len: 0,
            etag: Some("\"abc\"".into()),
            last_modified: None,
            expires_at: Some(now_ms() + 60_000),
            cacheable: true,
        }
    }

    #[tokio::test]
    async fn store_and_load_roundtrip() {
        let dir = std::env::temp_dir().join(format!("rc-disk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let disk = DiskCache::open(&dir).unwrap();
        let url = "http://example.com/a";
        let key = cache_key(url).as_str().to_string();
        disk.store(meta_for(url), b"hello").await.unwrap();
        let meta = disk.load_meta(&key).await.unwrap().unwrap();
        assert_eq!(meta.status, 200);
        let body = disk.load_body(&key).await.unwrap().unwrap();
        assert_eq!(body, b"hello");
        let (bytes, n) = disk.usage().await.unwrap();
        assert_eq!(bytes, 5);
        assert_eq!(n, 1);
        disk.remove(&key).await.unwrap();
        assert!(disk.load_body(&key).await.unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

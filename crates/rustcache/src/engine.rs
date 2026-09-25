//! Shared cache engine used by HTTP and MITM listeners.

use std::sync::Arc;

use rustcache_core::cache::coalesce::Coalesce;
use rustcache_core::cache::disk::DiskCache;
use rustcache_core::cache::evict::evict_lru;
use rustcache_core::cache::key::cache_key;
use rustcache_core::cache::mem::{CachedEntry, MemCache};
use rustcache_core::cache::meta::now_ms;
use rustcache_core::excl::ExclusionSet;
use rustcache_core::http::cache_policy::{CacheDecision, CachePolicy};
use rustcache_core::http::fetch::{OriginFetcher, OriginResponse};
use rustcache_core::stats::metrics::Metrics;
use rustcache_core::stats::ring::{ReqRecord, ReqRing};
use tokio::sync::RwLock;

pub type SharedEngine = Arc<CacheEngine>;

pub struct CacheEngine {
    pub disk: DiskCache,
    pub mem: MemCache,
    pub metrics: Arc<Metrics>,
    pub ring: Arc<ReqRing>,
    pub exclusions: RwLock<ExclusionSet>,
    pub fetcher: OriginFetcher,
    pub max_object_bytes: u64,
    pub max_bytes: u64,
    coalesce: Coalesce<OriginResponse>,
}

pub enum Lookup {
    Hit(CachedEntry),
    Revalidate { entry: CachedEntry },
    Miss,
}

impl CacheEngine {
    pub fn new(
        disk: DiskCache,
        mem: MemCache,
        exclusions: ExclusionSet,
        max_object_bytes: u64,
        max_bytes: u64,
    ) -> Self {
        Self {
            disk,
            mem,
            metrics: Metrics::shared(),
            ring: Arc::new(ReqRing::default()),
            exclusions: RwLock::new(exclusions),
            fetcher: OriginFetcher::default(),
            max_object_bytes,
            max_bytes,
            coalesce: Coalesce::new(),
        }
    }

    pub async fn is_excluded_url(&self, url: &str) -> bool {
        self.exclusions.read().await.is_excluded_url(url)
    }

    /// Requests with Authorization must not share cached responses.
    pub fn request_is_private(headers: &[(String, String)]) -> bool {
        headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("authorization"))
    }

    pub async fn lookup(&self, url: &str) -> Lookup {
        let key = cache_key(url);
        if let Some(e) = self.mem.get(key.as_str()).await {
            if e.meta.is_fresh(now_ms()) {
                let _ = self.disk.touch(key.as_str()).await;
                return Lookup::Hit(e);
            }
            return Lookup::Revalidate { entry: e };
        }
        if let Some(meta) = self.disk.load_meta(key.as_str()).await.ok().flatten() {
            let fresh = meta.is_fresh(now_ms());
            if let Some(body) = self.disk.load_body(key.as_str()).await.ok().flatten() {
                let entry = CachedEntry {
                    meta: meta.clone(),
                    body: Arc::new(body),
                };
                if fresh {
                    self.mem
                        .insert(
                            key.as_str(),
                            CachedEntry {
                                meta: entry.meta.clone(),
                                body: entry.body.clone(),
                            },
                            None,
                        )
                        .await;
                    let _ = self.disk.touch(key.as_str()).await;
                    return Lookup::Hit(entry);
                }
                return Lookup::Revalidate { entry };
            }
        }
        Lookup::Miss
    }

    /// Fetch from origin (or revalidate) and store. Coalesced per key.
    pub async fn fetch_and_store(
        &self,
        method: &str,
        url: &str,
        req_headers: &[(String, String)],
        body: Option<&[u8]>,
        tls: Option<Arc<tokio_rustls::TlsConnector>>,
        revalidate: Option<&CachedEntry>,
    ) -> anyhow::Result<CachedEntry> {
        let key = cache_key(url).as_str().to_string();
        let mut headers = req_headers.to_vec();
        let has_auth = headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("authorization"));
        if let Some(prev) = revalidate {
            if let Some(etag) = &prev.meta.etag {
                headers.push(("If-None-Match".into(), etag.clone()));
            } else if let Some(lm) = &prev.meta.last_modified {
                headers.push(("If-Modified-Since".into(), lm.clone()));
            }
        }

        let max_object_bytes = self.max_object_bytes;
        // Do not coalesce authenticated fetches (per-user responses).
        let coalesce_key = if has_auth {
            format!("{key}:auth:{}", headers.len())
        } else {
            key.clone()
        };

        let resp = self
            .coalesce
            .run(&coalesce_key, || async {
                self.fetcher
                    .fetch(method, url, &headers, body, tls, max_object_bytes)
                    .await
                    .map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| anyhow::anyhow!(e))?;

        if resp.status == 304 {
            if let Some(prev) = revalidate {
                self.metrics.add_revalidation();
                let mut meta = prev.meta.clone();
                meta.last_access = now_ms();
                // extend freshness from policy re-parse of original headers
                let policy = CachePolicy::from_headers(prev.meta.status, &prev.meta.headers);
                if let Some(ttl) = policy.ttl() {
                    meta.expires_at = Some(now_ms() + ttl.as_millis() as u64);
                }
                let entry = CachedEntry {
                    meta: meta.clone(),
                    body: prev.body.clone(),
                };
                self.mem
                    .insert(
                        &key,
                        CachedEntry {
                            meta: meta.clone(),
                            body: entry.body.clone(),
                        },
                        None,
                    )
                    .await;
                let _ = self.disk.store(meta, &entry.body).await;
                return Ok(entry);
            }
            // 304 without a stored entry: never cache it as a response body.
            return Err(anyhow::anyhow!(
                "origin returned 304 without a revalidation base"
            ));
        }

        let policy = CachePolicy::from_headers(resp.status, &resp.headers);
        // Only GET populates the cache. HEAD may read a GET entry but must not
        // store its empty body (that would poison later GETs).
        let decision = policy.decide(method.eq_ignore_ascii_case("GET") && !has_auth);
        let headers = resp.headers.clone();
        let meta = policy.to_meta(&key, url, resp.status, headers);
        let entry = CachedEntry {
            meta: meta.clone(),
            body: Arc::new(resp.body.clone()),
        };

        match decision {
            CacheDecision::NoStore | CacheDecision::Bypass => {
                // do not persist
            }
            _ => {
                self.mem
                    .insert(
                        &key,
                        CachedEntry {
                            meta: entry.meta.clone(),
                            body: entry.body.clone(),
                        },
                        policy.ttl(),
                    )
                    .await;
                let _ = self.disk.store(entry.meta.clone(), &entry.body).await;
                let _ = evict_lru(&self.disk, self.max_bytes).await;
            }
        }
        Ok(entry)
    }

    pub async fn purge_all(&self) -> anyhow::Result<u64> {
        self.mem.invalidate_all().await;
        let n = self.disk.purge_all().await?;
        Ok(n)
    }

    /// Drop a single URL from mem + disk. Returns true if a disk entry was removed.
    pub async fn purge_key(&self, url: &str) -> anyhow::Result<bool> {
        let key = cache_key(url);
        self.mem.invalidate(key.as_str()).await;
        Ok(self.disk.remove(key.as_str()).await?)
    }

    pub async fn cache_size(&self) -> anyhow::Result<(u64, u64)> {
        Ok(self.disk.usage().await?)
    }

    pub fn record(&self, rec: ReqRecord) {
        self.ring.push(rec);
    }

    pub fn metrics(&self) -> Arc<Metrics> {
        self.metrics.clone()
    }
}

/// Build a TLS connector for upstream HTTPS using webpki roots.
pub fn upstream_tls_connector() -> Arc<tokio_rustls::TlsConnector> {
    let mut root_store = rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let cfg = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    Arc::new(tokio_rustls::TlsConnector::from(Arc::new(cfg)))
}

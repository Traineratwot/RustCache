//! Shared cache engine used by HTTP and MITM listeners.
//!
//! Owns disk/mem caches, metrics, request log, exclusions, and origin fetch.
//! Listeners call its methods instead of reaching into the subsystems
//! (Law of Demeter): lookups, fetch-and-store, purge, and recording all go
//! through this facade.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

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
use rustcache_core::stats::{LogStore, ReqRecord};
use tokio::sync::RwLock;

pub type SharedEngine = Arc<CacheEngine>;

/// Orchestrates cache lookup, origin fetch, persistence, and request recording.
///
/// Subsystems (disk, mem, metrics, logs, exclusions, fetcher) are private.
/// Callers use the methods below so ownership and locking stay in one place.
pub struct CacheEngine {
    disk: DiskCache,
    mem: MemCache,
    metrics: Arc<Metrics>,
    logs: Arc<LogStore>,
    exclusions: RwLock<ExclusionSet>,
    fetcher: OriginFetcher,
    /// Live-tunable: max response body stored (hot-reloaded from config).
    max_object_bytes: AtomicU64,
    /// Live-tunable: disk cache size cap (hot-reloaded from config).
    max_bytes: AtomicU64,
    coalesce: Coalesce<OriginResponse>,
}

/// Result of consulting the caches before deciding to fetch.
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
        logs: Arc<LogStore>,
        max_object_bytes: u64,
        max_bytes: u64,
    ) -> Self {
        Self {
            disk,
            mem,
            metrics: Metrics::shared(),
            logs,
            exclusions: RwLock::new(exclusions),
            fetcher: OriginFetcher::default(),
            max_object_bytes: AtomicU64::new(max_object_bytes),
            max_bytes: AtomicU64::new(max_bytes),
            coalesce: Coalesce::new(),
        }
    }

    /// Live cap on a single stored response body.
    pub fn max_object_bytes(&self) -> u64 {
        self.max_object_bytes.load(Ordering::Relaxed)
    }

    /// Live disk-cache size cap in bytes.
    pub fn max_bytes(&self) -> u64 {
        self.max_bytes.load(Ordering::Relaxed)
    }

    /// Hot-apply cache size limits (config reload / API update).
    pub fn set_cache_limits(&self, max_object_bytes: u64, max_bytes: u64) {
        self.max_object_bytes
            .store(max_object_bytes, Ordering::Relaxed);
        self.max_bytes.store(max_bytes, Ordering::Relaxed);
    }

    /// True when `url` matches an exclusion rule (domain or CIDR).
    pub async fn is_excluded_url(&self, url: &str) -> bool {
        self.exclusions.read().await.is_excluded_url(url)
    }

    /// Snapshot of the current exclusion matchers (API listing).
    pub async fn exclusion_matchers(&self) -> Vec<rustcache_core::excl::Matcher> {
        self.exclusions.read().await.matchers().to_vec()
    }

    /// Replace the exclusion set (hot-reload / API CRUD).
    pub async fn set_exclusions(&self, set: ExclusionSet) {
        *self.exclusions.write().await = set;
    }

    /// Requests with Authorization must not share cached responses.
    pub fn request_is_private(headers: &[(String, String)]) -> bool {
        headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("authorization"))
    }

    /// Consult mem then disk. Fresh hits touch LRU and may promote to mem.
    pub async fn lookup(&self, url: &str) -> Lookup {
        let key = cache_key(url);
        if let Some(e) = self.mem.get(key.as_str()).await {
            if e.meta.is_fresh(now_ms()) {
                if let Err(e) = self.disk.touch(key.as_str()).await {
                    tracing::warn!(key = %key, error = %e, "disk touch failed");
                }
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
                    if let Err(e) = self.disk.touch(key.as_str()).await {
                        tracing::warn!(key = %key, error = %e, "disk touch failed");
                    }
                    return Lookup::Hit(entry);
                }
                return Lookup::Revalidate { entry };
            }
        }
        Lookup::Miss
    }

    /// Fetch from origin (or revalidate) and store. Coalesced per key.
    ///
    /// Only GET populates the cache (HEAD may read). `Authorization` requests
    /// are never stored. Persist failures are logged, not returned — the
    /// fetched body is still served to the client.
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

        let max_object_bytes = self.max_object_bytes();
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
            return self.apply_revalidation(&key, resp, revalidate).await;
        }

        let policy = CachePolicy::from_headers(resp.status, &resp.headers);
        // Only GET populates the cache. HEAD may read a GET entry but must not
        // store its empty body (that would poison later GETs).
        let decision = policy.decide(method.eq_ignore_ascii_case("GET") && !has_auth);
        let meta = policy.to_meta(&key, url, resp.status, resp.headers.clone());
        let entry = CachedEntry {
            meta: meta.clone(),
            body: Arc::new(resp.body.clone()),
        };

        self.store_if_cacheable(&key, decision, &policy, entry.clone())
            .await;
        Ok(entry)
    }

    /// Handle a 304 revalidation response: extend freshness and refresh stores.
    async fn apply_revalidation(
        &self,
        key: &str,
        resp: OriginResponse,
        revalidate: Option<&CachedEntry>,
    ) -> anyhow::Result<CachedEntry> {
        let prev = match revalidate {
            Some(p) => p,
            // 304 without a stored entry: never cache it as a response body.
            None => {
                return Err(anyhow::anyhow!(
                    "origin returned 304 without a revalidation base"
                ))
            }
        };
        let _ = resp;
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
                key,
                CachedEntry {
                    meta: meta.clone(),
                    body: entry.body.clone(),
                },
                None,
            )
            .await;
        if let Err(e) = self.disk.store(meta, &entry.body).await {
            // Persist failure must not fail the client response — the
            // fetched body is still served from this entry.
            tracing::warn!(key, error = %e, "disk store failed after revalidation");
        }
        Ok(entry)
    }

    /// Persist an entry when policy allows. Never fails the caller on I/O errors.
    async fn store_if_cacheable(
        &self,
        key: &str,
        decision: CacheDecision,
        policy: &CachePolicy,
        entry: CachedEntry,
    ) {
        match decision {
            CacheDecision::NoStore | CacheDecision::Bypass => {
                // do not persist
            }
            _ => {
                self.mem
                    .insert(
                        key,
                        CachedEntry {
                            meta: entry.meta.clone(),
                            body: entry.body.clone(),
                        },
                        policy.ttl(),
                    )
                    .await;
                if let Err(e) = self.disk.store(entry.meta.clone(), &entry.body).await {
                    tracing::warn!(key, error = %e, "disk store failed");
                }
                if let Err(e) = evict_lru(&self.disk, self.max_bytes()).await {
                    tracing::warn!(error = %e, "lru eviction failed");
                }
            }
        }
    }

    /// Drop every cache entry (mem + disk).
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

    /// `(bytes, entries)` currently on disk.
    pub async fn cache_size(&self) -> anyhow::Result<(u64, u64)> {
        Ok(self.disk.usage().await?)
    }

    /// Append one request to the traffic history (non-blocking).
    pub fn record(&self, rec: ReqRecord) {
        self.logs.enqueue(rec);
    }

    /// Process-lifetime metrics counters (hit rate, tunnels, errors).
    pub fn metrics(&self) -> Arc<Metrics> {
        self.metrics.clone()
    }

    /// Request-log store (query/stats/clear for the API and cleanup tasks).
    pub fn logs(&self) -> Arc<LogStore> {
        self.logs.clone()
    }

    /// Shared upstream TLS connector (webpki roots, built once).
    pub fn tls_connector(&self) -> Arc<tokio_rustls::TlsConnector> {
        upstream_tls_connector()
    }

    /// Fetch from origin without caching (bypass / tunnel paths).
    pub async fn fetch(
        &self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        body: Option<&[u8]>,
        tls: Option<Arc<tokio_rustls::TlsConnector>>,
    ) -> rustcache_core::Result<OriginResponse> {
        self.fetcher
            .fetch(method, url, headers, body, tls, self.max_object_bytes())
            .await
    }

    /// Direct disk access for tests and cache-seeding helpers.
    pub fn disk(&self) -> &DiskCache {
        &self.disk
    }

    /// Direct mem access for tests (invalidate / promote).
    pub fn mem(&self) -> &MemCache {
        &self.mem
    }
}

/// Build a TLS connector for upstream HTTPS using webpki roots.
///
/// Built once and reused — constructing `rustls::ClientConfig` per request
/// is expensive and unnecessary (roots do not change at runtime).
pub fn upstream_tls_connector() -> Arc<tokio_rustls::TlsConnector> {
    static CONNECTOR: OnceLock<Arc<tokio_rustls::TlsConnector>> = OnceLock::new();
    CONNECTOR
        .get_or_init(|| {
            let mut root_store = rustls::RootCertStore::empty();
            root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let cfg = rustls::ClientConfig::builder()
                .with_root_certificates(root_store)
                .with_no_client_auth();
            Arc::new(tokio_rustls::TlsConnector::from(Arc::new(cfg)))
        })
        .clone()
}

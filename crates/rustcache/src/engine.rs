//! Shared cache engine used by HTTP and MITM listeners.
//!
//! Owns disk/mem caches, metrics, request log, exclusions, and origin fetch.
//! Listeners call its methods instead of reaching into the subsystems
//! (Law of Demeter): lookups, fetch-and-store, purge, and recording all go
//! through this facade.

use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

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
    disk: Arc<DiskCache>,
    mem: MemCache,
    metrics: Arc<Metrics>,
    logs: Arc<LogStore>,
    exclusions: RwLock<ExclusionSet>,
    fetcher: OriginFetcher,
    /// Live-tunable: max response body stored (hot-reloaded from config).
    max_object_bytes: AtomicU64,
    /// Live-tunable: disk cache size cap (hot-reloaded from config).
    max_bytes: AtomicU64,
    /// Live-tunable: serve stale + background revalidate (hot-reloaded).
    optimistic: AtomicBool,
    coalesce: Coalesce<OriginResponse>,
    /// Background disk/revalidate tasks (for `flush_bg` in tests and purge).
    bg: Mutex<Vec<tokio::task::JoinHandle<()>>>,
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
            disk: Arc::new(disk),
            mem,
            metrics: Metrics::shared(),
            logs,
            exclusions: RwLock::new(exclusions),
            fetcher: OriginFetcher::default(),
            max_object_bytes: AtomicU64::new(max_object_bytes),
            max_bytes: AtomicU64::new(max_bytes),
            optimistic: AtomicBool::new(true),
            coalesce: Coalesce::new(),
            bg: Mutex::new(Vec::new()),
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

    /// Serve stale entries while revalidating in the background.
    pub fn optimistic(&self) -> bool {
        self.optimistic.load(Ordering::Relaxed)
    }

    /// Hot-apply optimistic caching (config reload / API update).
    pub fn set_optimistic(&self, on: bool) {
        self.optimistic.store(on, Ordering::Relaxed);
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
                self.touch_bg(key.as_str().to_string());
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
                    self.touch_bg(key.as_str().to_string());
                    return Lookup::Hit(entry);
                }
                return Lookup::Revalidate { entry };
            }
        }
        Lookup::Miss
    }

    /// Spawn work that must not block the client response. Tracked for flush.
    pub fn spawn_bg<F>(&self, fut: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let handle = tokio::spawn(fut);
        if let Ok(mut g) = self.bg.lock() {
            g.retain(|h| !h.is_finished());
            g.push(handle);
        }
    }

    /// Await all tracked background disk/revalidate tasks.
    pub async fn flush_bg(&self) {
        loop {
            let handles: Vec<_> = {
                let mut g = match self.bg.lock() {
                    Ok(g) => g,
                    Err(e) => e.into_inner(),
                };
                if g.is_empty() {
                    break;
                }
                g.drain(..).collect()
            };
            for h in handles {
                let _ = h.await;
            }
        }
    }

    /// LRU touch must not delay the client response.
    fn touch_bg(&self, key: String) {
        let disk = self.disk.clone();
        self.spawn_bg(async move {
            if let Err(e) = disk.touch(&key).await {
                tracing::warn!(key = %key, error = %e, "disk touch failed");
            }
        });
    }

    /// Disk write + LRU eviction must not delay the client response.
    /// Mem insert stays on the hot path so concurrent lookups see the entry.
    fn persist_bg(&self, meta: rustcache_core::cache::meta::CacheMeta, body: Arc<Vec<u8>>) {
        let disk = self.disk.clone();
        let max_bytes = self.max_bytes();
        self.spawn_bg(async move {
            if let Err(e) = disk.store(meta, &body).await {
                tracing::warn!(error = %e, "disk store failed");
            }
            if let Err(e) = evict_lru(&disk, max_bytes).await {
                tracing::warn!(error = %e, "lru eviction failed");
            }
        });
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

        // Authenticated fetches are never coalesced: the response is specific
        // to one set of credentials. The previous key (`{key}:auth:{n}`) only
        // varied by header *count*, so two users with the same number of
        // headers asking for the same URL shared one upstream response.
        let resp = if has_auth {
            self.fetcher
                .fetch(method, url, &headers, body, tls, max_object_bytes)
                .await?
        } else {
            self.coalesce
                .run(&key, || async {
                    self.fetcher
                        .fetch(method, url, &headers, body, tls, max_object_bytes)
                        .await
                        .map_err(|e| e.to_string())
                })
                .await
                .map_err(|e| anyhow::anyhow!(e))?
        };

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
                ));
            }
        };
        self.metrics.add_revalidation();

        // The 304 carries the origin's *new* freshness information. Re-parsing
        // the stored headers instead (as this used to) replayed the original,
        // already-expired `Date`/`max-age`, so every single request went back
        // to the origin even though it had just said "not modified".
        let has_fresh_date = resp
            .headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("date"));
        let headers = merge_revalidated_headers(&prev.meta.headers, &resp.headers);
        let policy = CachePolicy::from_headers(prev.meta.status, &headers);
        let now = now_ms();
        let mut meta = policy.to_meta(key, &prev.meta.url, prev.meta.status, headers);
        meta.body_len = prev.meta.body_len;
        meta.last_access = now;

        // `max-age` counts from the response `Date`. When the 304 omits it we
        // are left with the stored one, which is exactly as old as the entry —
        // so the "refreshed" entry came back already expired and every request
        // turned into a conditional round-trip. Freshness restarts at receipt
        // time instead (RFC 9111 §4.2.3). `max-age=0` still yields `now`, i.e.
        // revalidate again next time, which is what the origin asked for.
        if !has_fresh_date {
            if let Some(ma) = policy.s_maxage.or(policy.max_age) {
                meta.expires_at = Some(now + ma.saturating_mul(1000));
            }
        }
        // Validators only, no freshness information anywhere: keep the entry
        // usable briefly rather than revalidating on the very next request.
        // Not when the origin explicitly demands revalidation — `no-cache` and
        // `must-revalidate` mean exactly "check with me every time".
        let must_recheck = policy.no_cache || policy.must_revalidate;
        if meta.expires_at.is_none() && !must_recheck {
            meta.expires_at = Some(now + REVALIDATED_MIN_FRESH_MS);
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
                Some(std::time::Duration::from_secs(600)),
            )
            .await;
        self.persist_bg(meta, entry.body.clone());
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
                // Keep zero-TTL entries in mem so Revalidate/optimistic can
                // find them without waiting for the disk write.
                let mem_ttl = match policy.ttl() {
                    Some(d) if !d.is_zero() => Some(d),
                    _ => Some(std::time::Duration::from_secs(600)),
                };
                self.mem
                    .insert(
                        key,
                        CachedEntry {
                            meta: entry.meta.clone(),
                            body: entry.body.clone(),
                        },
                        mem_ttl,
                    )
                    .await;
                self.persist_bg(entry.meta, entry.body);
            }
        }
    }

    /// Drop every cache entry (mem + disk).
    pub async fn purge_all(&self) -> anyhow::Result<u64> {
        self.flush_bg().await;
        self.mem.invalidate_all().await;
        let n = self.disk.purge_all().await?;
        Ok(n)
    }

    /// Drop a single URL from mem + disk. Returns true if a disk entry was removed.
    pub async fn purge_key(&self, url: &str) -> anyhow::Result<bool> {
        self.flush_bg().await;
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
        self.disk.as_ref()
    }

    /// Direct mem access for tests (invalidate / promote).
    pub fn mem(&self) -> &MemCache {
        &self.mem
    }
}

/// Headers a `304 Not Modified` is allowed to refresh on the stored entry
/// (RFC 9111 §4.3.4). Everything else keeps the stored value.
const REVALIDATION_HEADERS: [&str; 6] = [
    "cache-control",
    "date",
    "etag",
    "expires",
    "last-modified",
    "vary",
];

/// Freshness floor applied after a successful revalidation that carried no
/// usable `Cache-Control`/`Expires`. Without it, a validator-only origin makes
/// every request a conditional round-trip.
const REVALIDATED_MIN_FRESH_MS: u64 = 60_000;

/// Overlay the 304's freshness/validator headers onto the stored ones.
fn merge_revalidated_headers(
    stored: &[(String, String)],
    fresh: &[(String, String)],
) -> Vec<(String, String)> {
    let updated: Vec<&(String, String)> = fresh
        .iter()
        .filter(|(k, _)| {
            REVALIDATION_HEADERS
                .iter()
                .any(|h| k.eq_ignore_ascii_case(h))
        })
        .collect();
    let mut out: Vec<(String, String)> = stored
        .iter()
        .filter(|(k, _)| !updated.iter().any(|(uk, _)| uk.eq_ignore_ascii_case(k)))
        .cloned()
        .collect();
    out.extend(updated.into_iter().cloned());
    out
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

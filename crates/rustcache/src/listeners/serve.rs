//! Shared cache-serve pipeline for the HTTP and MITM listeners.
//!
//! Listeners parse the request, build a [`RequestContext`], call
//! [`resolve_cached`], and write the returned [`CacheOutcome`]. Every
//! Hit / Revalidate / Miss / Bypass / Error decision, metric update, and
//! request-log record lives here so the listeners stay dispatchers and the
//! two paths cannot diverge on cache semantics.

use std::sync::Arc;
use std::time::Instant;

use rustcache_core::cache::mem::CachedEntry;
use rustcache_core::stats::{Outcome, ReqRecord};

use crate::engine::{CacheEngine, Lookup, SharedEngine};

use super::wire::{cache_headers, entry_age_secs};

/// A request ready to be served through the shared cache pipeline.
pub struct RequestContext {
    /// HTTP method, uppercased (`GET`, `HEAD`, …).
    pub method: String,
    /// Absolute request URL (absolute-form on :3128, `https://host/path` on MITM).
    pub url: String,
    /// Origin host without port — recorded in the request log.
    pub host: String,
    /// Request headers as parsed. `Authorization` presence forces cache bypass.
    pub headers: Vec<(String, String)>,
    /// Request body, already capped by [`super::wire::read_http_request`].
    pub body: Vec<u8>,
    /// When the listener started this request — drives `duration_ms`.
    pub started: Instant,
}

/// A fully prepared client response covering every cache outcome.
///
/// Callers only write: metrics and the request-log record are already done.
pub struct CacheOutcome {
    /// HTTP status to write.
    pub status: u16,
    /// Response headers including the `X-RustCache-*` diagnostics.
    pub headers: Vec<(String, String)>,
    /// Body bytes to send to the client. Empty for HEAD; the full length is
    /// still reported in [`Self::resp_bytes`].
    pub body: Arc<[u8]>,
    /// Cache outcome — request log and `X-RustCache-Status`.
    pub outcome: Outcome,
    /// Logical response size in bytes (full body length, even for HEAD).
    pub resp_bytes: u64,
}

/// Shared Hit / Revalidate / Miss / Bypass pipeline.
///
/// Handles the full decision tree and records metrics + one [`ReqRecord`].
/// Origin failures become an `Ok` outcome with status 502 and
/// [`Outcome::Error`] so callers have a single write path.
///
/// `tls` is forwarded to the origin fetch and is only consulted for `https://`
/// URLs; plain HTTP ignores it.
pub async fn resolve_cached(
    engine: &SharedEngine,
    ctx: &RequestContext,
    tls: Option<Arc<tokio_rustls::TlsConnector>>,
) -> anyhow::Result<CacheOutcome> {
    let is_get_head = ctx.method == "GET" || ctx.method == "HEAD";
    // Auth, exclusions, and non-GET/HEAD never touch the cache: fetch origin
    // and do not store (GET-only populates; HEAD may read a GET entry).
    let bypass = !is_get_head
        || engine.is_excluded_url(&ctx.url).await
        || CacheEngine::request_is_private(&ctx.headers);

    let out = if bypass {
        resolve_bypass(engine, ctx, tls).await
    } else {
        resolve_lookup(engine, ctx, tls).await
    };
    engine.record(record_for(ctx, &out));
    Ok(out)
}

/// Unified revalidation rule: [`Outcome::HitRevalidated`] iff etag AND body
/// match the previously stored entry; anything else that replaced the entry is
/// [`Outcome::Revalidated`].
pub fn revalidate_outcome(old: &CachedEntry, new: &CachedEntry) -> Outcome {
    if old.meta.etag == new.meta.etag && old.body == new.body {
        Outcome::HitRevalidated
    } else {
        Outcome::Revalidated
    }
}

/// Pass-through for auth / excluded / non-cacheable methods.
async fn resolve_bypass(
    engine: &SharedEngine,
    ctx: &RequestContext,
    tls: Option<Arc<tokio_rustls::TlsConnector>>,
) -> CacheOutcome {
    engine.metrics().add_bypass();
    let body = if ctx.body.is_empty() {
        None
    } else {
        Some(ctx.body.as_slice())
    };
    match engine
        .fetch(&ctx.method, &ctx.url, &ctx.headers, body, tls)
        .await
    {
        Ok(r) => {
            let headers = cache_headers(&r.headers, "BYPASS", None);
            let body = client_body(&ctx.method, &r.body);
            engine.metrics().add_served(body.len() as u64);
            CacheOutcome {
                status: r.status,
                headers,
                body,
                outcome: Outcome::Bypass,
                resp_bytes: r.body.len() as u64,
            }
        }
        Err(e) => error_outcome(engine, &e.to_string()),
    }
}

/// Cache lookup → Hit / Revalidate / Miss for GET and HEAD.
async fn resolve_lookup(
    engine: &SharedEngine,
    ctx: &RequestContext,
    tls: Option<Arc<tokio_rustls::TlsConnector>>,
) -> CacheOutcome {
    match engine.lookup(&ctx.url).await {
        Lookup::Hit(entry) => {
            engine.metrics().add_hit(entry.body.len() as u64);
            cached_outcome(engine, ctx, entry, Outcome::Hit)
        }
        Lookup::Revalidate { entry } => {
            match engine
                .fetch_and_store(&ctx.method, &ctx.url, &ctx.headers, None, tls, Some(&entry))
                .await
            {
                Ok(new_entry) => {
                    let outcome = revalidate_outcome(&entry, &new_entry);
                    if outcome.is_hit() {
                        engine.metrics().add_hit(new_entry.body.len() as u64);
                    } else {
                        engine.metrics().add_miss();
                    }
                    cached_outcome(engine, ctx, new_entry, outcome)
                }
                Err(e) => error_outcome(engine, &e.to_string()),
            }
        }
        Lookup::Miss => {
            match engine
                .fetch_and_store(&ctx.method, &ctx.url, &ctx.headers, None, tls, None)
                .await
            {
                Ok(entry) => {
                    engine.metrics().add_miss();
                    cached_outcome(engine, ctx, entry, Outcome::Miss)
                }
                Err(e) => error_outcome(engine, &e.to_string()),
            }
        }
    }
}

/// Wrap a cached entry as the client response (HEAD keeps full length in
/// `resp_bytes` but sends no body).
fn cached_outcome(
    engine: &SharedEngine,
    ctx: &RequestContext,
    entry: CachedEntry,
    outcome: Outcome,
) -> CacheOutcome {
    let body = client_body(&ctx.method, &entry.body);
    engine.metrics().add_served(body.len() as u64);
    let headers = cache_headers(
        &entry.meta.headers,
        outcome.as_str(),
        Some(entry_age_secs(entry.meta.stored_at)),
    );
    CacheOutcome {
        status: entry.meta.status,
        headers,
        body,
        outcome,
        resp_bytes: entry.body.len() as u64,
    }
}

/// Origin / pipeline failure → 502 with `X-RustCache-Status: ERROR`.
fn error_outcome(engine: &SharedEngine, msg: &str) -> CacheOutcome {
    engine.metrics().add_error();
    CacheOutcome {
        status: 502,
        headers: cache_headers(&[], "ERROR", None),
        body: Arc::from(msg.as_bytes()),
        outcome: Outcome::Error,
        resp_bytes: 0,
    }
}

/// Body bytes sent to the client. HEAD responses carry headers only.
fn client_body(method: &str, full: &[u8]) -> Arc<[u8]> {
    if method.eq_ignore_ascii_case("HEAD") {
        Arc::from(&[][..])
    } else {
        Arc::from(full)
    }
}

/// The single request-log record for a served request.
fn record_for(ctx: &RequestContext, out: &CacheOutcome) -> ReqRecord {
    ReqRecord {
        ts: rustcache_core::cache::meta::now_ms(),
        method: ctx.method.clone(),
        url: ctx.url.clone(),
        host: ctx.host.clone(),
        status: out.status,
        outcome: out.outcome,
        duration_ms: ctx.started.elapsed().as_millis() as u64,
        resp_bytes: out.resp_bytes,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rustcache_core::cache::mem::CachedEntry;
    use rustcache_core::cache::meta::CacheMeta;
    use rustcache_core::stats::Outcome;

    use super::revalidate_outcome;

    fn entry(etag: Option<&str>, body: &[u8]) -> CachedEntry {
        CachedEntry {
            meta: CacheMeta {
                key: "k".into(),
                url: "http://example.com/".into(),
                status: 200,
                headers: vec![],
                stored_at: 0,
                last_access: 0,
                body_len: body.len() as u64,
                etag: etag.map(str::to_string),
                last_modified: None,
                expires_at: None,
                cacheable: true,
            },
            body: Arc::new(body.to_vec()),
        }
    }

    #[test]
    fn revalidate_same_etag_and_body_is_hit_revalidated() {
        assert_eq!(
            revalidate_outcome(&entry(Some("\"e1\""), b"v1"), &entry(Some("\"e1\""), b"v1")),
            Outcome::HitRevalidated
        );
    }

    #[test]
    fn revalidate_etag_only_is_revalidated() {
        assert_eq!(
            revalidate_outcome(&entry(Some("\"e1\""), b"v1"), &entry(Some("\"e1\""), b"v2")),
            Outcome::Revalidated
        );
    }

    #[test]
    fn revalidate_body_only_is_revalidated() {
        assert_eq!(
            revalidate_outcome(&entry(Some("\"e1\""), b"v1"), &entry(Some("\"e2\""), b"v1")),
            Outcome::Revalidated
        );
    }

    #[test]
    fn revalidate_both_differ_is_revalidated() {
        assert_eq!(
            revalidate_outcome(&entry(Some("\"e1\""), b"v1"), &entry(Some("\"e2\""), b"v2")),
            Outcome::Revalidated
        );
    }
}

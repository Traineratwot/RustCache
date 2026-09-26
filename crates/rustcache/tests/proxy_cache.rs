//! Integration: HTTP proxy cache path — MISS/HIT, revalidate, no-store, coalesce, exclusions, purge.

mod common;

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use common::*;
use rustcache_core::cache::key::cache_key;
use rustcache_core::excl::ExclusionSet;

#[tokio::test]
async fn miss_then_hit_identical_body() {
    install_crypto();
    let origin_state = OriginState::new("hello-cache", "max-age=60");
    let origin = spawn_origin(origin_state.clone()).await;
    let (engine, dir) = spawn_engine(ExclusionSet::default()).await;
    let proxy = spawn_http_proxy(engine.clone()).await;

    let url = format!("http://127.0.0.1:{}/item", origin.port());
    let t0 = Instant::now();
    let (s1, b1) = proxy_get(proxy, &url).await.unwrap();
    let d1 = t0.elapsed();
    assert_eq!(s1, 200);
    assert_eq!(b1, b"hello-cache");

    let t1 = Instant::now();
    let (s2, b2) = proxy_get(proxy, &url).await.unwrap();
    let d2 = t1.elapsed();
    assert_eq!(s2, 200);
    assert_eq!(b2, b"hello-cache");
    assert_eq!(origin_state.hits(), 1);
    assert!(
        d2 <= d1 + Duration::from_millis(50),
        "hit={d2:?} miss={d1:?}"
    );

    let snap = engine.metrics().snapshot();
    assert_eq!(snap.misses, 1);
    assert_eq!(snap.hits, 1);

    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn max_age_expiry_revalidate_304_hit_revalidated() {
    install_crypto();
    let origin_state = OriginState::new("v1", "max-age=0");
    *origin_state.etag.write().unwrap() = Some("\"e1\"".into());
    let origin = spawn_origin(origin_state.clone()).await;
    let (engine, dir) = spawn_engine(ExclusionSet::default()).await;
    let proxy = spawn_http_proxy(engine.clone()).await;

    let url = format!("http://127.0.0.1:{}/reval", origin.port());
    let (s1, b1) = proxy_get(proxy, &url).await.unwrap();
    assert_eq!(s1, 200);
    assert_eq!(b1, b"v1");
    assert_eq!(origin_state.hits(), 1);

    // max-age=0 → immediately stale → revalidate with If-None-Match
    let (s2, b2) = proxy_get(proxy, &url).await.unwrap();
    assert_eq!(s2, 200);
    assert_eq!(b2, b"v1");
    assert_eq!(
        origin_state.hits(),
        2,
        "second request revalidates at origin"
    );

    let reqs = origin_state.requests.read().unwrap().clone();
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[1].2.as_deref(), Some("\"e1\""), "If-None-Match sent");

    let snap = engine.metrics().snapshot();
    assert!(
        snap.revalidations >= 1,
        "expected revalidation, got {snap:?}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn no_store_never_cached() {
    install_crypto();
    let origin_state = OriginState::new("secret", "no-store");
    let origin = spawn_origin(origin_state.clone()).await;
    let (engine, dir) = spawn_engine(ExclusionSet::default()).await;
    let proxy = spawn_http_proxy(engine.clone()).await;

    let url = format!("http://127.0.0.1:{}/nostore", origin.port());
    let (s1, b1) = proxy_get(proxy, &url).await.unwrap();
    assert_eq!(s1, 200);
    assert_eq!(b1, b"secret");
    let (s2, b2) = proxy_get(proxy, &url).await.unwrap();
    assert_eq!(s2, 200);
    assert_eq!(b2, b"secret");
    assert_eq!(origin_state.hits(), 2, "no-store must always hit origin");
    let key = cache_key(&url);
    assert!(
        engine
            .disk()
            .load_meta(key.as_str())
            .await
            .unwrap()
            .is_none()
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn coalescing_parallel_same_url_single_origin_fetch() {
    install_crypto();
    let origin_state = OriginState::new("coalesced", "max-age=60");
    origin_state.delay_ms.store(80, Ordering::SeqCst);
    let origin = spawn_origin(origin_state.clone()).await;
    let (engine, dir) = spawn_engine(ExclusionSet::default()).await;

    let url = format!("http://127.0.0.1:{}/slow", origin.port());
    let mut handles = Vec::new();
    for _ in 0..8 {
        let engine = engine.clone();
        let url = url.clone();
        handles.push(tokio::spawn(async move {
            engine
                .fetch_and_store("GET", &url, &[], None, None, None)
                .await
                .unwrap()
        }));
    }
    for h in handles {
        let e = h.await.unwrap();
        assert_eq!(e.body.as_slice(), b"coalesced");
    }
    assert_eq!(
        origin_state.hits(),
        1,
        "N parallel misses must share one origin fetch"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn exclusion_bypasses_cache() {
    install_crypto();
    let origin_state = OriginState::new("excluded", "max-age=60");
    let origin = spawn_origin(origin_state.clone()).await;
    let (engine, dir) = spawn_engine(ExclusionSet::from_specs(&["127.0.0.1".into()], &[])).await;
    let proxy = spawn_http_proxy(engine.clone()).await;

    let url = format!("http://127.0.0.1:{}/excl", origin.port());
    let (s1, _) = proxy_get(proxy, &url).await.unwrap();
    assert_eq!(s1, 200);
    let (s2, _) = proxy_get(proxy, &url).await.unwrap();
    assert_eq!(s2, 200);
    assert_eq!(origin_state.hits(), 2, "excluded host must never cache");
    let snap = engine.metrics().snapshot();
    assert_eq!(snap.bypasses, 2);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn exclusion_cidr_bypasses() {
    install_crypto();
    let origin_state = OriginState::new("cidr", "max-age=60");
    let origin = spawn_origin(origin_state.clone()).await;
    let (engine, dir) = spawn_engine(ExclusionSet::from_specs(&[], &["127.0.0.0/8".into()])).await;
    let proxy = spawn_http_proxy(engine.clone()).await;

    let url = format!("http://127.0.0.1:{}/cidr", origin.port());
    let _ = proxy_get(proxy, &url).await.unwrap();
    let _ = proxy_get(proxy, &url).await.unwrap();
    assert_eq!(origin_state.hits(), 2);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn authorization_bypasses_cache() {
    install_crypto();
    let origin_state = OriginState::new("auth", "max-age=60");
    let origin = spawn_origin(origin_state.clone()).await;
    let (engine, dir) = spawn_engine(ExclusionSet::default()).await;

    let url = format!("http://127.0.0.1:{}/auth", origin.port());
    let headers = vec![("Authorization".to_string(), "Bearer x".to_string())];
    let _ = engine
        .fetch_and_store("GET", &url, &headers, None, None, None)
        .await
        .unwrap();
    let _ = engine
        .fetch_and_store("GET", &url, &headers, None, None, None)
        .await
        .unwrap();
    assert_eq!(origin_state.hits(), 2, "Authorization must bypass cache");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn purge_all_and_by_key() {
    install_crypto();
    let origin_state = OriginState::new("purge-me", "max-age=60");
    let origin = spawn_origin(origin_state.clone()).await;
    let (engine, dir) = spawn_engine(ExclusionSet::default()).await;
    let proxy = spawn_http_proxy(engine.clone()).await;

    let url = format!("http://127.0.0.1:{}/p", origin.port());
    let (s1, _) = proxy_get(proxy, &url).await.unwrap();
    assert_eq!(s1, 200);
    let key = cache_key(&url);
    assert!(
        engine
            .disk()
            .load_meta(key.as_str())
            .await
            .unwrap()
            .is_some()
    );

    // by key
    assert!(engine.purge_key(&url).await.unwrap());
    assert!(
        engine
            .disk()
            .load_meta(key.as_str())
            .await
            .unwrap()
            .is_none()
    );
    let _ = proxy_get(proxy, &url).await.unwrap();
    assert_eq!(origin_state.hits(), 2);

    // purge all
    let n = engine.purge_all().await.unwrap();
    assert!(n >= 1);
    let (bytes, entries) = engine.cache_size().await.unwrap();
    assert_eq!(entries, 0);
    assert_eq!(bytes, 0);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn hot_reload_exclusions() {
    install_crypto();
    let origin_state = OriginState::new("hot", "max-age=60");
    let origin = spawn_origin(origin_state.clone()).await;
    let (engine, dir) = spawn_engine(ExclusionSet::default()).await;
    let proxy = spawn_http_proxy(engine.clone()).await;

    let url = format!("http://127.0.0.1:{}/hot", origin.port());
    let _ = proxy_get(proxy, &url).await.unwrap();
    let _ = proxy_get(proxy, &url).await.unwrap();
    assert_eq!(origin_state.hits(), 1, "cached before exclusion");

    engine
        .set_exclusions(ExclusionSet::from_specs(&["127.0.0.1".into()], &[]))
        .await;

    let _ = proxy_get(proxy, &url).await.unwrap();
    let _ = proxy_get(proxy, &url).await.unwrap();
    assert_eq!(origin_state.hits(), 3, "after reload: bypass every time");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn disk_fallback_hit_after_mem_invalidated() {
    install_crypto();
    let origin_state = OriginState::new("disk-hit", "max-age=60");
    let origin = spawn_origin(origin_state.clone()).await;
    let (engine, dir) = spawn_engine(ExclusionSet::default()).await;
    let proxy = spawn_http_proxy(engine.clone()).await;

    let url = format!("http://127.0.0.1:{}/disk", origin.port());
    let _ = proxy_get(proxy, &url).await.unwrap();
    engine.mem().invalidate_all().await;
    let _ = proxy_get(proxy, &url).await.unwrap();
    assert_eq!(origin_state.hits(), 1, "disk entry serves after mem miss");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn response_headers_report_version_and_cache_status() {
    install_crypto();
    let origin_state = OriginState::new("hdr", "max-age=60");
    let origin = spawn_origin(origin_state.clone()).await;
    let (engine, dir) = spawn_engine(ExclusionSet::default()).await;
    let proxy = spawn_http_proxy(engine.clone()).await;

    let url = format!("http://127.0.0.1:{}/hdr", origin.port());

    let (s1, h1, b1) = proxy_get_full(proxy, &url).await.unwrap();
    assert_eq!(s1, 200);
    assert_eq!(b1, b"hdr");
    assert_eq!(
        header(&h1, "X-RustCache-Version"),
        Some(rustcache_core::version())
    );
    assert_eq!(header(&h1, "X-RustCache-Status"), Some("MISS"));
    assert!(header(&h1, "X-RustCache-Age").is_some());

    let (s2, h2, b2) = proxy_get_full(proxy, &url).await.unwrap();
    assert_eq!(s2, 200);
    assert_eq!(b2, b"hdr");
    assert_eq!(
        header(&h2, "X-RustCache-Version"),
        Some(rustcache_core::version())
    );
    assert_eq!(header(&h2, "X-RustCache-Status"), Some("HIT"));
    assert!(header(&h2, "X-RustCache-Age").is_some());

    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn bypass_response_reports_bypass_status() {
    install_crypto();
    let origin_state = OriginState::new("byp", "max-age=60");
    let origin = spawn_origin(origin_state.clone()).await;
    let (engine, dir) = spawn_engine(ExclusionSet::from_specs(&["127.0.0.1".into()], &[])).await;
    let proxy = spawn_http_proxy(engine.clone()).await;

    let url = format!("http://127.0.0.1:{}/byp", origin.port());
    let (s, h, _) = proxy_get_full(proxy, &url).await.unwrap();
    assert_eq!(s, 200);
    assert_eq!(header(&h, "X-RustCache-Status"), Some("BYPASS"));
    assert_eq!(
        header(&h, "X-RustCache-Version"),
        Some(rustcache_core::version())
    );

    let _ = std::fs::remove_dir_all(dir);
}

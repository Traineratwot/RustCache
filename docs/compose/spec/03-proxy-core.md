---
feature: 03-proxy-core
status: in-progress
updated: 2026-09-25
branch: feat/03-proxy-core
commits: 393f106..
---

# RustCache Proxy Core (Plan 03)

Source plan: `docs/plans/03-implementation.md`. This document tracks delivery of the plan’s exit criteria.

## Report

## [S1] Problem

RustCache is an empty cargo workspace. Users need a local caching proxy that serves cached HTTP/HTTPS responses, tunnels what it should not cache, issues MITM certificates from a local CA, exposes runtime stats/config over REST, and stays within the project safety rules (no secret logging, CA key 0600, hex-only cache keys, API on localhost).

## [S2] Design

Behavior and contracts are defined by `docs/plans/03-implementation.md` (architecture, module map, disk layout, ports, API table, config schema, implementation order). Key contracts:

- **Ports:** 3128 HTTP proxy, 3129 HTTPS MITM, 1080 SOCKS5, 8080 API/UI bind `127.0.0.1`.
- **Cache key:** `hex(blake3(canonical URL))` under `index/ab/cd/<key>.json` + `objects/ab/cd/<key>.body` with atomic tmp-rename.
- **Cache path:** absolute-URI / MITM HTTP → exclusions → mem→disk lookup → HIT / revalidate 304 / MISS + single-flight → stream client + disk. Eviction LRU by `last_access` when `cache.max_bytes` exceeded. Mem cache is optimization; disk is source of truth.
- **CONNECT on :3128:** raw tunnel (`copy_bidirectional`), no cache.
- **MITM on :3129:** CONNECT → rustls server with on-the-fly leaf (SAN=host, signed by local CA) → same cache path; upstream rustls + webpki-roots. Excluded hosts splice (no MITM). Upgrade/WebSocket bypass cache. Protocol errors denylist the host at runtime.
- **SOCKS5:** no-auth, CONNECT only, raw tunnel, tunnel/byte/duration stats.
- **CA:** root at `ca/ca.crt` (0644) + `ca/ca.key` (**0600**), CN=`RustCache MITM Root`. CLI `gen-ca` / `export-ca`, plus `GET /api/ca.crt`.
- **Stats:** AtomicU64 counters + ring buffer of 100 `ReqRecord`s.
- **Exclusions:** exact / `*.suffix` wildcard / domain suffix + CIDR, hot-swappable via `ArcSwap`.
- **REST API:** table in the plan (`/api/health`, `/api/stats`, `/api/requests`, `/api/config`, `/api/config/reload`, `/api/exclusions` CRUD, `/api/ca.crt`, `/api/cache`, SPA `/*` when embedded).
- **CLI:** `run | gen-ca | export-ca | purge` via clap; tracing-subscriber env-filter. Never log Authorization/Cookie/query secrets.
- **Hot-reload:** notify on config; ports restart listeners; exclusions/cache apply live.

Project rules (AGENTS.md) are binding: no unwrap/expect in production paths, CA key 0600, hex-only cache keys, API binds 127.0.0.1.

## [S3] Out of Scope

- React / Vite web UI and `embed-ui` feature completion (plan 05).
- Full integration test suite and `scripts/smoke.sh` as a gated deliverable (plan 04). Plan 03 includes the manual end-to-end smoke from §3.9 as acceptance evidence only.
- HTTP/2 ALPN on MITM (phase 2 in the plan).
- SOCKS5 UDP / BIND.
- Upstream certificate pinning, auth to the proxy, or clustering.

## Tasks

- [ ] T1: Workspace deps + config schema + CLI skeleton + logging — acceptance: `cargo build` succeeds; `rustcache --help` lists `run|gen-ca|export-ca|purge`; config example parses. (covers: S2)
- [ ] T2: rustcache-core cache (key, disk, mem, meta, evict, coalesce) — acceptance: unit-testable modules exist; hex-only keys; atomic store/load; LRU cap; single-flight API. (covers: S2)
- [ ] T3: cache policy + origin fetch + HTTP proxy listener — acceptance: absolute-URI proxy path does exclusion → lookup → HIT/MISS/revalidate and records stats. (covers: S2)
- [ ] T4: stats metrics + request ring — acceptance: counters increment; last 100 `ReqRecord`s queryable. (covers: S2)
- [ ] T5: exclusions matchers (exact/wildcard/suffix + CIDR) — acceptance: matchers reject and bypass as specified; reloadable. (covers: S2)
- [ ] T6: CONNECT tunnel on HTTP port — acceptance: CONNECT pipes bytes without cache. (covers: S2)
- [ ] T7: SOCKS5 CONNECT tunnel — acceptance: no-auth CONNECT works; UDP/Bind rejected; tunnel stats recorded. (covers: S2)
- [ ] T8: CA gen/load + leaf cache + HTTPS MITM listener — acceptance: `gen-ca` writes 0600 key; MITM caches https:// with trusted CA; excluded host spliced; Upgrade bypassed. (covers: S2)
- [ ] T9: REST API + purge + hot-reload — acceptance: all plan API routes respond; purge clears cache; reload applies exclusions. (covers: S2)
- [ ] T10: End-to-end verification per plan §3.9 + clippy/fmt — acceptance: exit criteria of plan 03 observed or explicitly marked unmet with reason. (covers: S2)

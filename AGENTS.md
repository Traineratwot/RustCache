# RustCache — agent guide

Caching HTTP/HTTPS proxy (MITM + disk/mem cache) with SOCKS5 tunnel and local REST API.
Plans: `docs/plans/`. Delivery reports: `docs/compose/spec/`.

## Stack

- **Rust** workspace (edition 2024, MSRV 1.87): tokio + hyper/hyper-util + axum (API) + rustls/tokio-rustls + rcgen (CA/leaf) + moka (mem) + rusqlite (request log) + blake3 (cache keys) + notify (config watch)
- **Web UI** (`web/`): React 19 + TypeScript + Vite + react-router-dom + PrimeReact/PrimeFlex/primeicons + Biome (`lint`/`fmt`)
- TLS upstream trust: `webpki-roots` only (no system roots)

## Layout

- `crates/rustcache-core` — lib: `cache/` (key/disk/mem/meta/evict/coalesce), `certs/` (ca/leaf/store), `http/` (cache_policy/fetch), `stats/` (metrics + SQLite `logstore`), `excl/`
- `crates/rustcache` — bin: CLI `main.rs`, shared `engine.rs` (`CacheEngine`), `listeners/` (http_proxy, mitm_proxy, socks5), `api/` (routes, pac, state), `config/` (schema, watch), `ui_embed.rs` (feature `embed-ui`)
- `web/` — SPA source. `scripts/smoke.sh` and integration `tests/` live under `crates/rustcache/tests/` (fixtures in `tests/common/`).

## Settings & data locations

| What | Where |
|------|--------|
| Config | `config.toml` (gitignored) — copy from `config.example.toml`. Missing file → built-in defaults. |
| Data root | `data_dir` (default `~/.local/share/rustcache`) — CLI `--data-dir DIR` overrides |
| Cache dir | `$data_dir/cache` (`[cache] dir`, relative paths resolve under `data_dir`) |
| Root CA | `$data_dir/ca/` (`[ca] dir`) — `ca.crt` + `ca.key` (0600) |
| Request log DB | `$data_dir/logs.db` (`[logs] db_path`), SQLite WAL, file mode 0600 |

Config sections: `data_dir` + `[http]` `[https]` `[socks5]` `[api]` `[cache]` `[exclude]` `[ca]` `[pac]` `[logs]`.
Relative `dir`/`db_path` values resolve under `data_dir`; absolute paths (or `~/...`) are used as-is.
Tilde paths expand via `Config::expand_tilde`. `config.example.toml` is the schema reference.

## Logs

Two separate streams — do not confuse them:

1. **Runtime tracing** → **stdout/stderr only** (no log file). `RUST_LOG` env filter, default `info`. Set `RUST_LOG=debug` when debugging.
2. **Request log** (traffic history for the UI) → SQLite `logs.db` via `rustcache_core::stats::LogStore`. Single writer thread + non-blocking enqueue (drops on full channel, warns on powers of two so a burst cannot drown the log). Retention: `max_rows` (default 10k), `max_age_days` (default 7), periodic `cleanup_interval_secs` (default 300). Query/filter API: `GET /api/requests`.
   `LogStore::enqueue` runs every URL through `stats::redact_url` — credential-looking query parameters (`token`, `api_key`, `signature`, …) keep their name but lose their value. It is the single choke point; do not bypass it by writing rows directly.

## Web UI structure

`web/src/`:
- `pages/` — Dashboard (`/`), Health, Requests, Exclusions, Cache, Connect, Settings, Ca
- `layout/Layout.tsx` — sidebar nav (labels are Russian)
- `api/client.ts` + `api/types.ts` — typed fetch wrappers for `/api/*`
- `styles.css` — hand-rolled CSS (not Tailwind; the plan mentioned Tailwind but it was not adopted)

Dev server proxies `/api` → `http://127.0.0.1:8080` (`web/vite.config.ts`).
Production: `npm --prefix web run build` → `web/dist`, embedded into the binary with `--features embed-ui` (`ui_embed.rs` serves SPA with `index.html` fallback).

REST surface (see `crates/rustcache/src/api/routes.rs`): `/api/health` `/api/stats` `/api/requests` `/api/logs/settings` `/api/config` `/api/config/reload` `/api/config/restart` `/api/exclusions` `/api/ca.crt` `/api/cache` `/api/netinfo` `/api/pac` + `/proxy.pac` `/wpad.dat`.
PAC listener (default `0.0.0.0:8081`, `[pac]`) is LAN-facing and serves **only** the two PAC paths — no admin endpoints.

The admin router (not `pac_router`) is wrapped in `api::guard::local_origin_guard`: a request is rejected with 403 when its `Origin` is cross-site, or when its `Host` is a hostname that is neither loopback nor the configured `api.bind` host. That is what keeps a random web page from calling `POST /api/config/restart` (no body, no preflight — browsers really do send it) and what blocks DNS rebinding. IP-literal `Host` values stay allowed, so reaching the UI over the LAN still works. Covered by `tests/api_guard.rs`.

## Commands

```bash
cargo build
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --check

cargo run -p rustcache -- run --config config.toml   # starts all listeners + API
cargo run -p rustcache -- run --data-dir /var/lib/rustcache   # override data root
cargo run -p rustcache -- gen-ca                     # optional: run auto-creates CA via load_ca
cargo run -p rustcache -- export-ca --pem
cargo run -p rustcache -- purge                      # clear on-disk cache

# single test
cargo test -p rustcache-core cache::key

# web
npm --prefix web run dev      # Vite + HMR, proxies /api to :8080
npm --prefix web run build    # tsc -b && vite build → web/dist
npm --prefix web run lint     # biome check src
npm --prefix web run fmt      # biome format --write src
```

Gate before claiming done: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`.
(`--all-targets` matters: without it the integration tests under `crates/rustcache/tests/` are never linted.)
For UI work also: `npm --prefix web run lint && npm --prefix web run build`.

## Ports (defaults)

3128 HTTP proxy · 3129 HTTPS MITM · 1080 SOCKS5 · 8080 API (bind `127.0.0.1`) · 8081 PAC (bind `0.0.0.0`, LAN)

## Hard rules (do not "fix")

- No `unwrap`/`expect` outside tests
- CA key `ca.key` must stay 0600 (enforced at create and on load)
- Never log Authorization / Cookie / secrets in query strings
- Cache keys are hex blake3 only — enforced on every disk path (`is_hex_key`)
- Only GET populates the cache; HEAD may read a GET entry, never store
- Requests with `Authorization` bypass the cache entirely
- API binds 127.0.0.1 by default, and the admin router keeps its local-origin guard
- Request-log URLs stay redacted — `LogStore::enqueue` is the only writer path
- Every network read has a deadline (`listeners::HANDSHAKE_TIMEOUT` / `CONNECT_TIMEOUT`, `OriginFetcher::read_timeout`); do not reintroduce an unbounded read
- **All `config.toml` settings must be manageable from the web UI.** Every field in `config.example.toml` / `config/schema.rs` has a matching control in `web/src/pages/Settings.tsx` and is saved via `PUT /api/config` (exclusions use `PUT /api/exclusions`). When you add a config key, add it to: schema, example.toml, Settings form, `validate_config`/`restart_fields_diff`, and `web/src/api/types.ts`.

## Gotchas

- `.gitignore` must keep `/cache/` and `/ca/` with the leading slash. Bare `cache/` silently ignored `crates/rustcache-core/src/cache/` sources.
- Do not hold `parking_lot::Mutex` across `.await` — futures become non-`Send`. Use `tokio::sync::Mutex` for async sections (disk write lock already does).
- Upstream TLS trusts `webpki-roots` only: a local/private TLS origin fails MITM fetch (502). Smoke with a public host.
- Rebuild `target/debug/rustcache` before retesting — a stale binary masks fixes.
- Config hot-reload applies exclusions, cache limits (`max_bytes` / `max_object_bytes`) and log retention; ports, binds and paths need a restart. `PUT /api/config` writes the whole `config.toml`, hot-applies, and returns `restart_fields` — it does **not** restart. `POST /api/config/restart` re-execs the process (same argv); `?dry_run=1` is for tests. On self-restart the child waits for `RUSTCACHE_RESTARTED_FROM` to exit before binding.
- `PUT /api/config` validates before write: ranges/bind format, **internal port conflicts** (http/https/socks5/api/pac must be unique), port availability (changed ports only — current ports are ours until restart), and path type + create/write permissions. Returns `errors: [{field, message}]` so the Settings form can highlight fields.
- Health reports actual bind success (`ListenerStatus`), not a TCP probe — two services on one misconfigured port used to both show "running".
- `Vary` is parsed but is not part of the cache key (known limitation).
- A `304` refreshes the entry from the **304's** headers (`engine::merge_revalidated_headers`), and when the 304 carries no `Date` the `max-age` window restarts at receipt. Re-deriving freshness from the stored headers replays an already-elapsed window, which turns every later request into a conditional round-trip (`tests/proxy_cache.rs::revalidation_restores_freshness_instead_of_staying_stale`).
- Authenticated requests (`Authorization`) are **never** coalesced — the response belongs to one set of credentials.
- MITM leaf certs are valid ~397 days: Apple platforms reject server certs above 398 days, and rcgen's default (`1975..4096`) fails every handshake on macOS/iOS. `CertificateParams::new` also classifies IP-literal hosts as `iPAddress` SANs — do not overwrite `subject_alt_names` with a bare `DnsName`.
- The MITM denylist entry is a timestamp with a 5-minute TTL, not a permanent flag. Per-host `rustls::ServerConfig`s are cached in `MitmState::tls_configs`; building one per CONNECT is expensive because the MITM path serves one request per TLS connection.
- `wait_for_restart_parent` probes with `kill(pid, 0)`, not `/proc` — `/proc` does not exist on macOS/BSD, so the old check always reported "parent gone" and the restart raced the ports.
- `write_config_toml` is tmp-file + rename. A plain truncate-write races the notify watcher, which re-parses `config.toml` the instant it changes.
- Hop-by-hop request headers are stripped before the origin fetch (`http::fetch::HOP_BY_HOP` plus whatever the client listed in `Connection:`), and `Connection: close` is always sent. Forwarding `keep-alive` made the read-to-EOF path wait out the origin's idle timeout.
- SOCKS5 is no-auth CONNECT-only tunnel — no caching on that path. CONNECT on :3128 is also a raw tunnel.
- Excluded hosts on the MITM port are spliced (raw tunnel, no MITM) — client sees the origin cert.
- `embed-ui` needs `web/dist` present at compile time (`rust-embed` folder `../../web/dist/`). Build the UI first or the feature fails.

## Smoke (manual curl)

```bash
curl -x http://127.0.0.1:3128 http://example.com/   # twice: MISS then HIT
curl --cacert ~/.local/share/rustcache/ca/ca.crt -x http://127.0.0.1:3129 https://example.com/
curl --socks5 127.0.0.1:1080 https://example.com/
curl http://127.0.0.1:8080/api/stats
bash scripts/smoke.sh   # scripted version of the above
```

Expect HIT/MISS, `bytes_saved` grows, CA key mode 0600.

## Project skills

`.mimocode/skills/rustcache-dev` and `.mimocode/skills/proxy-smoke-test` cover build/run and smoke workflows.

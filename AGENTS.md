# RustCache — agent guide

Caching HTTP/HTTPS proxy (MITM + disk/mem cache) with SOCKS5 tunnel and local REST API.
Plans: `docs/plans/`. Delivery reports: `docs/compose/spec/`.

## Stack

- **Rust** workspace (edition 2021): tokio + hyper/hyper-util + axum (API) + rustls/tokio-rustls + rcgen (CA/leaf) + moka (mem) + rusqlite (request log) + blake3 (cache keys) + notify (config watch)
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
| Cache dir | `~/.local/share/rustcache/cache` (`[cache] dir`) |
| Root CA | `~/.local/share/rustcache/ca/` (`[ca] dir`) — `ca.crt` + `ca.key` (0600) |
| Request log DB | `~/.local/share/rustcache/logs.db` (`[logs] db_path`), SQLite WAL, file mode 0600 |

Config sections: `[http]` `[https]` `[socks5]` `[api]` `[cache]` `[exclude]` `[ca]` `[pac]` `[logs]`.
Tilde paths expand via `Config::expand_tilde`. `config.example.toml` is the schema reference.

## Logs

Two separate streams — do not confuse them:

1. **Runtime tracing** → **stdout/stderr only** (no log file). `RUST_LOG` env filter, default `info`. Set `RUST_LOG=debug` when debugging.
2. **Request log** (traffic history for the UI) → SQLite `logs.db` via `rustcache_core::stats::LogStore`. Single writer thread + non-blocking enqueue (drops on full channel). Retention: `max_rows` (default 10k), `max_age_days` (default 7), periodic `cleanup_interval_secs` (default 300). Query/filter API: `GET /api/requests`.

## Web UI structure

`web/src/`:
- `pages/` — Dashboard (`/`), Health, Requests, Exclusions, Cache, Connect, Settings, Ca
- `layout/Layout.tsx` — sidebar nav (labels are Russian)
- `api/client.ts` + `api/types.ts` — typed fetch wrappers for `/api/*`
- `styles.css` — hand-rolled CSS (not Tailwind; the plan mentioned Tailwind but it was not adopted)

Dev server proxies `/api` → `http://127.0.0.1:8080` (`web/vite.config.ts`).
Production: `npm --prefix web run build` → `web/dist`, embedded into the binary with `--features embed-ui` (`ui_embed.rs` serves SPA with `index.html` fallback).

REST surface (see `crates/rustcache/src/api/routes.rs`): `/api/health` `/api/stats` `/api/requests` `/api/logs/settings` `/api/config` `/api/config/reload` `/api/exclusions` `/api/ca.crt` `/api/cache` `/api/netinfo` `/api/pac` + `/proxy.pac` `/wpad.dat`.
PAC listener (default `0.0.0.0:8081`, `[pac]`) is LAN-facing and serves **only** the two PAC paths — no admin endpoints.

## Commands

```bash
cargo build
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --check

cargo run -p rustcache -- run --config config.toml   # starts all listeners + API
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

Gate before claiming done: `cargo fmt --check && cargo clippy --workspace -- -D warnings && cargo test --workspace`.
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
- API binds 127.0.0.1 by default

## Gotchas

- `.gitignore` must keep `/cache/` and `/ca/` with the leading slash. Bare `cache/` silently ignored `crates/rustcache-core/src/cache/` sources.
- Do not hold `parking_lot::Mutex` across `.await` — futures become non-`Send`. Use `tokio::sync::Mutex` for async sections (disk write lock already does).
- Upstream TLS trusts `webpki-roots` only: a local/private TLS origin fails MITM fetch (502). Smoke with a public host.
- Rebuild `target/debug/rustcache` before retesting — a stale binary masks fixes.
- Config hot-reload applies exclusions only; ports and cache limits need a restart. API writes to `config.toml` on exclusion/log-settings changes (whole file rewritten).
- `Vary` is parsed but is not part of the cache key (known limitation).
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

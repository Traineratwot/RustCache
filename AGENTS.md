# RustCache — agent guide

Caching HTTP/HTTPS proxy (MITM + disk/mem cache) with SOCKS5 tunnel and local REST API.
Plans: `docs/plans/`. Delivery reports: `docs/compose/spec/`.

## Layout

- `crates/rustcache-core` — lib: `cache/` (key/disk/mem/meta/evict/coalesce), `certs/` (ca/leaf), `http/` (cache_policy/fetch), `stats/`, `excl/`
- `crates/rustcache` — bin: CLI `main.rs`, shared `engine.rs` (`CacheEngine`), `listeners/` (http_proxy, mitm_proxy, socks5), `api/`, `config/`
- `config.example.toml` → copy to `config.toml` (gitignored). Missing config falls back to built-in defaults (`~/.local/share/rustcache/{ca,cache}`).
- `web/` (plan 05). `scripts/smoke.sh` and integration `tests/` live under `crates/rustcache/tests/`. The `embed-ui` feature is declared in Cargo.toml but `ui_embed.rs` does not exist — don't invent it.

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
```

Gate before claiming done: `cargo fmt --check && cargo clippy --workspace -- -D warnings && cargo test --workspace`.

## Ports (defaults)

3128 HTTP proxy · 3129 HTTPS MITM · 1080 SOCKS5 · 8080 API (bind `127.0.0.1`)

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
- Config hot-reload applies exclusions only; ports and cache limits need a restart.
- `Vary` is parsed but is not part of the cache key (known limitation).
- SOCKS5 is no-auth CONNECT-only tunnel — no caching on that path. CONNECT on :3128 is also a raw tunnel.
- Excluded hosts on the MITM port are spliced (raw tunnel, no MITM) — client sees the origin cert.

## Smoke (manual curl)

```bash
curl -x http://127.0.0.1:3128 http://example.com/   # twice: MISS then HIT
curl --cacert ~/.local/share/rustcache/ca/ca.crt -x http://127.0.0.1:3129 https://example.com/
curl --socks5 127.0.0.1:1080 https://example.com/
curl http://127.0.0.1:8080/api/stats
```

Expect HIT/MISS, `bytes_saved` grows, CA key mode 0600.

## Project skills

`.mimocode/skills/rustcache-dev` and `.mimocode/skills/proxy-smoke-test` cover build/run and smoke workflows.

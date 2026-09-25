# RustCache — agent guide

## Stack
- Rust workspace: crates/rustcache (bin), crates/rustcache-core (lib)
- React + Vite + TS в web/
- Tokio, hyper, axum, rustls, rcgen, moka

## Commands
- cargo build / cargo test / cargo clippy -- -D warnings / cargo fmt
- cargo run -p rustcache -- gen-ca
- cargo run -p rustcache -- run --config config.toml
- npm --prefix web run build
- scripts/smoke.sh

## Rules
- No unwrap/expect in production paths
- CA key must be 0600
- Never log Authorization / Cookie / secrets in query
- Cache keys are hex only (blake3) — no path traversal
- API binds 127.0.0.1 by default

## Ports (default)
- 3128 HTTP proxy
- 3129 HTTPS proxy (MITM)
- 1080 SOCKS5
- 8080 Web UI + REST API

## Proxy smoke
curl -x http://127.0.0.1:3128 http://example.com/
curl --cacert ~/.local/share/rustcache/ca/ca.crt -x http://127.0.0.1:3129 https://example.com/
curl --socks5 127.0.0.1:1080 https://example.com/
curl http://127.0.0.1:8080/api/stats

# RustCache

Caching proxy server for HTTP/HTTPS traffic, with SOCKS5 support and a Web UI.

## Default ports

| Port | Service |
|------|---------|
| 3128 | HTTP proxy |
| 3129 | HTTPS-MITM proxy |
| 1080 | SOCKS5 |
| 8080 | API / Web UI |

## Build

```bash
cargo build --release
```

## Development

```bash
cargo build
cargo run -p rustcache
cargo fmt --check
cargo clippy -- -D warnings
```

## Documentation

Project plans live in [`docs/plans/`](docs/plans/).

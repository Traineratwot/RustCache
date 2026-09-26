# RustCache

Caching HTTP/HTTPS proxy (MITM + disk/mem cache) with a SOCKS5 tunnel and a local REST API + Web UI.

## Features

- **HTTP proxy** on `:3128` — absolute-form GET/HEAD served from cache (mem → disk), miss/revalidate via origin
- **HTTPS MITM** on `:3129` — on-the-fly leaf certs signed by a local CA; excluded hosts are raw-tunneled
- **SOCKS5** on `:1080` — CONNECT-only tunnel (no caching)
- **REST API + SPA** on `127.0.0.1:8080` — health, request log, cache, exclusions, config, CA download
- **PAC / WPAD** on `:8081` (optional) — LAN-facing, serves only PAC files

## Comparison

Honest feature matrix against common alternatives. `Yes` / `Partial` / `No`.

| Feature | RustCache | Squid | nginx (proxy_cache) | mitmproxy | Privoxy |
|---------|-----------|-------|---------------------|-----------|---------|
| HTTP forward-proxy cache | Yes | Yes | Partial [^1] | No | No |
| HTTPS caching via MITM | Yes | Partial [^2] | No | Partial [^3] | No |
| HTTPS CONNECT tunnel (no decrypt) | Yes | Yes | Partial [^4] | Yes | Yes |
| SOCKS5 tunnel | Yes | No | No | Partial [^5] | Partial [^6] |
| Built-in Web UI + REST API | Yes | Partial [^7] | No | Yes [^8] | No |
| Request log / hit-rate analytics | Yes | Partial [^9] | Partial [^9] | Yes | Partial [^9] |
| PAC / WPAD serving | Yes | No | No | No | No |
| Local root CA management | Yes | Partial [^2] | No | Yes | No |
| ACL / URL filtering rules | Partial [^10] | Yes | Yes | Yes | Yes |
| Client proxy authentication | No | Yes | Yes | Partial | Partial |
| Cache hierarchy / peer clustering | No | Yes | No | No | Partial [^11] |
| Content adaptation (ICAP / eCAP) | No | Yes | No | Partial [^12] | Partial [^12] |
| Reverse proxy / acceleration | No | Yes | Yes | Partial [^13] | No |
| Mem + disk cache, stale-while-revalidate | Yes | Yes | Yes | No | No |
| Config from Web UI + hot reload | Yes | No | No | Partial | Partial |

[^1]: nginx `proxy_cache` is a reverse-proxy cache; forward-proxy mode needs extra modules and is not the primary use case.
[^2]: Squid `ssl_bump` can MITM HTTPS and cache decrypted bodies, but requires an OpenSSL build, certificate plumbing, and careful ACL setup.
[^3]: mitmproxy intercepts and rewrites traffic for debugging/security work; it is not a production HTTP cache.
[^4]: HTTP CONNECT passthrough in nginx needs third-party modules (e.g. `ngx_http_proxy_connect_module`).
[^5]: mitmproxy can talk to SOCKS upstreams and has reverse/upstream modes; it is not a general-purpose SOCKS5 server.
[^6]: Privoxy is primarily an HTTP filtering proxy; SOCKS is used on the parent-proxy chain, not as a first-class server mode.
[^7]: Squid ships `cachemgr.cgi` and access logs; a full analytics SPA is external (Lightsquid, sarg, etc.).
[^8]: mitmproxy has a web UI for inspecting flows, not for operating a shared cache.
[^9]: File-based access logs are standard; live hit-rate dashboards are not built in (except RustCache and mitmproxy's flow UI).
[^10]: RustCache exclusions are domain globs + CIDR bypass lists, not a general ACL language (no regex actions, time-based rules, or user groups).
[^11]: Privoxy can chain to a parent proxy; there is no ICP/HTCP cache mesh.
[^12]: Filtering/rewriting via scripts or actions, not the ICAP/eCAP protocol.
[^13]: mitmproxy has a reverse-proxy mode for local development, not origin acceleration at scale.

**Honest limitations of RustCache today**

- No client authentication and no per-user ACL language — Squid is the clear choice for multi-tenant enterprise policy
- `Vary` is parsed but is **not** part of the cache key
- Only `GET` populates the cache; requests with `Authorization` bypass the cache entirely
- No cache hierarchy / ICP / HTCP clustering and no ICAP content adaptation
- Upstream TLS trusts `webpki-roots` only (no system roots or custom CA bundle)
- Younger than Squid (~30 years of production hardening) and nginx — expect rough edges

**When to pick Squid instead:** you need proxy authentication, a real ACL language, transparent interception, cache peering/hierarchy, ICAP, or a reverse-proxy accelerator in one battle-tested daemon. **When to pick RustCache:** a single binary for a workstation or small LAN that caches HTTP *and* HTTPS (via local MITM), with a Web UI, request log, PAC, and config hot-reload out of the box.

## Screenshots

**Overview** — hit rate, traffic over time, outcomes, top hosts

![Overview](docs/media/screen_4.png)

**Requests** — live request log with HIT / MISS / ERROR badges

![Requests](docs/media/screen_2.png)

**Settings** — every `config.toml` key from the UI (hot / restart badges)

![Settings](docs/media/screen_1.png)

**Connect** — LAN addresses, PAC / WPAD URL, per-platform setup

![Connect](docs/media/screen_3.png)

## Default ports

| Port | Service |
|------|---------|
| 3128 | HTTP proxy |
| 3129 | HTTPS MITM proxy |
| 1080 | SOCKS5 |
| 8080 | API / Web UI (127.0.0.1) |
| 8081 | PAC (0.0.0.0, optional) |

## Install

Prebuilt packages for every release: [GitHub Releases](https://github.com/Traineratwot/RustCache/releases) (`.deb`, `.tar.gz`, `SHA256SUMS`) and multi-arch Docker images on [GHCR](https://github.com/Traineratwot/RustCache/pkgs/container/rustcache).

### Debian / Ubuntu (APT)

Public APT repo on GitHub Pages ([traineratwot.github.io/RustCache](https://traineratwot.github.io/RustCache/)):

```bash
echo "deb [trusted=yes] https://traineratwot.github.io/RustCache/ ./" \
  | sudo tee /etc/apt/sources.list.d/rustcache.list
sudo apt update
sudo apt install rustcache
```

The package installs a systemd unit and starts on boot:

```bash
sudo systemctl status rustcache
# config: /etc/rustcache/config.toml   data: /var/lib/rustcache
# logs:   journalctl -u rustcache      UI:   http://127.0.0.1:8080
```

### `.deb` directly

```bash
# pick amd64 or arm64 from the release page
wget https://github.com/Traineratwot/RustCache/releases/download/v0.1.0/rustcache_0.1.0_amd64.deb \
     https://github.com/Traineratwot/RustCache/releases/download/v0.1.0/SHA256SUMS
sha256sum --ignore-missing -c SHA256SUMS
sudo dpkg -i rustcache_0.1.0_amd64.deb
```

### Standalone binary (`.tar.gz`)

```bash
wget https://github.com/Traineratwot/RustCache/releases/download/v0.1.0/rustcache_0.1.0_linux_amd64.tar.gz
tar -xzf rustcache_0.1.0_linux_amd64.tar.gz
cd rustcache-0.1.0-linux-amd64
./rustcache run --config config.example.toml   # or edit a copy first
```

The archive ships `rustcache` + `config.example.toml`. No root required; missing config falls back to defaults.

### Docker

Image: [`ghcr.io/traineratwot/rustcache`](https://github.com/Traineratwot/RustCache/pkgs/container/rustcache) (`amd64` + `arm64`).

```bash
docker run -d --name rustcache \
  -p 3128:3128 -p 3129:3129 -p 1080:1080 \
  -p 8080:8080 -p 8081:8081 \
  -v rustcache-data:/var/lib/rustcache \
  ghcr.io/traineratwot/rustcache:latest
```

Config is baked in as `/etc/rustcache/config.toml`. Override it or use the Web UI (Settings):

```bash
docker run -d --name rustcache \
  -p 3128:3128 -p 3129:3129 -p 1080:1080 -p 8080:8080 -p 8081:8081 \
  -v rustcache-data:/var/lib/rustcache \
  -v "$PWD/config.toml:/etc/rustcache/config.toml:ro" \
  ghcr.io/traineratwot/rustcache:latest
```

### Build from source

```bash
# UI (required before embed-ui)
npm --prefix web install
npm --prefix web run build

# Binary (embeds web/dist)
cargo build --release --features embed-ui

# Or without embedded UI (serve web/dist separately)
cargo build --release
```

## Configuration

Settings live in a single TOML file. Missing `config.toml` falls back to built-in defaults. Template: [`config.example.toml`](config.example.toml) — every key is also editable in the Web UI at `http://127.0.0.1:8080` (Settings).

| Install | Config | Data root |
|---------|--------|-----------|
| deb / Docker | `/etc/rustcache/config.toml` | `/var/lib/rustcache` |
| tar.gz / source | `config.toml` (cwd) or `--config` | `~/.local/share/rustcache` (or `data_dir`) |

Paths in the config (`cache.dir`, `ca.dir`, `logs.db_path`) are relative to `data_dir` unless absolute or `~/...`. CLI `--data-dir DIR` overrides `data_dir` for the process.

```toml
data_dir = "~/.local/share/rustcache"

[http]    # HTTP proxy port (default 3128)
[https]   # HTTPS MITM proxy port (default 3129)
[socks5]  # SOCKS5 tunnel port (default 1080)
[api]     # REST API + Web UI bind (default 127.0.0.1:8080)
[cache]   # dir, max_bytes, max_object_bytes
[exclude] # domains (globs) + CIDRs bypass the cache / MITM
[ca]      # root CA directory (ca.crt + ca.key 0600)
[logs]    # SQLite request log: max_rows, max_age_days, cleanup_interval_secs
[pac]     # enabled, bind (default 0.0.0.0:8081), mode: http | socks | http+socks
```

Full annotated example: [`config.example.toml`](config.example.toml).

### Hot reload vs restart

- **Hot** (UI / file watch / `POST /api/config/reload`): exclusions, `cache.max_bytes`, `cache.max_object_bytes`, log retention.
- **Restart required**: ports, binds, `data_dir`, `cache.dir`, `ca.dir`, `logs.db_path`, `pac.enabled` / `pac.bind`.
- The Settings form marks each field. Saving writes `config.toml` and applies what it can; it does **not** restart. Use the restart button (`POST /api/config/restart`) or `systemctl restart rustcache` after port/path changes.

### HTTPS MITM trust

HTTPS interception uses a local root CA. Trust it on the client (or exclude sensitive hosts via `[exclude]`):

```bash
# from the UI: http://127.0.0.1:8080/api/ca.crt
rustcache export-ca --pem          # print PEM
# deb/Docker data root:
#   /var/lib/rustcache/ca/ca.crt
```

### CLI

```bash
rustcache run --config config.toml   # start listeners + API
rustcache gen-ca                     # create root CA if missing
rustcache export-ca [--pem]          # path or PEM of the root CA
rustcache purge                      # clear on-disk cache
# global: --data-dir DIR
```

### Client setup

Point the system / browser proxy at `http://<host>:3128` (HTTP+HTTPS) or use SOCKS5 `socks5://<host>:1080`. PAC/WPAD (if enabled): `http://<host>:8081/proxy.pac` — details and per-platform steps in the Web UI **Connect** page.

## Development

```bash
cargo fmt --check
cargo clippy --workspace -- -D warnings
cargo test --workspace

npm --prefix web run dev      # Vite + HMR, proxies /api → :8080
npm --prefix web run lint
npm --prefix web run build
```

## Translations

Web UI is available in English (`en`, source) and Russian (`ru`). Locale files live in `web/src/i18n/locales/*.json` (i18next, nested JSON).

Translations are managed on Weblate: **https://weblate.traineratwot.site/projects/rustcache/**

- Component: [`rustcache/web-ui`](https://weblate.traineratwot.site/projects/rustcache/web-ui/)
- Source strings: `web/src/i18n/locales/en.json`
- Add a language or fix a string in the Weblate UI — no need to open a PR for pure translation changes

[![Translation status](https://weblate.traineratwot.site/widgets/rustcache/-/web-ui/multi-auto.svg)](https://weblate.traineratwot.site/engage/rustcache/)

## Layout

| Path | Role |
|------|------|
| `crates/rustcache-core` | Cache key/disk/mem, certs, origin fetch, exclusions, request log |
| `crates/rustcache` | CLI, listeners (http/mitm/socks5), axum API, config, engine |
| `web/` | React 19 + TypeScript SPA |
| `packaging/` | Debian package, systemd unit, system `config.toml` |
| `pages/` | APT-repo landing page (GitHub Pages) |
| `docs/plans/` | Historical implementation plans (superseded by code where they drifted) |

## Data locations

Under `data_dir` (user default `~/.local/share/rustcache`, system package `/var/lib/rustcache`):

| What | Where |
|------|-------|
| Config | `config.toml` (or `--config`) — system: `/etc/rustcache/config.toml` |
| Data root | `data_dir` |
| Cache | `$data_dir/cache` |
| Root CA | `$data_dir/ca/` (`ca.key` mode 0600) |
| Request log | `$data_dir/logs.db` (SQLite WAL) |

## Smoke test

```bash
bash scripts/smoke.sh
# or manually:
curl -x http://127.0.0.1:3128 http://example.com/   # twice: MISS then HIT
curl --cacert ~/.local/share/rustcache/ca/ca.crt -x http://127.0.0.1:3129 https://example.com/
curl http://127.0.0.1:8080/api/stats
```

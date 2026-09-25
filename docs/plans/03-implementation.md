# План 3 — Реализация без интерфейса

**Проект:** RustCache — кеширующий proxy-сервер.
**Каталог:** `/home/kirill/PhpstormProjects/RustCache`
**Цель:** полный proxy + кеш + CA + статистика + REST API (без React).

**Предыдущий:** `docs/plans/02-ai-setup.md`
**Следующий:** `docs/plans/04-tests.md`

---

## Архитектура

### Структура workspace

```
RustCache/
├── Cargo.toml                  # workspace
├── crates/
│   ├── rustcache/              # бинарник: CLI, listeners, axum API, embed UI
│   │   └── src/
│   │       ├── main.rs         # clap: run | gen-ca | export-ca | purge
│   │       ├── listeners/{http_proxy,mitm_proxy,socks5}.rs
│   │       ├── api/{routes,state}.rs
│   │       ├── config/{schema,watch}.rs
│   │       └── ui_embed.rs     # rust-embed web/dist (feature embed-ui)
│   └── rustcache-core/         # библиотека: cache, certs, stats, exclusions
│       └── src/
│           ├── cache/{key,disk,mem,meta,evict,coalesce}.rs
│           ├── certs/{ca,leaf,store}.rs
│           ├── http/{cache_policy,fetch}.rs
│           ├── stats/{ring,metrics}.rs
│           └── excl/matchers.rs
├── web/                        # React + Vite + TypeScript (план 5)
├── config.example.toml
└── tests/                      # интеграционные (план 4)
```

### Ключевые зависимости

| Crate | Назначение |
|---|---|
| `tokio` 1.x | async runtime |
| `hyper` 1.x + `hyper-util` | HTTP/1.1 клиент/сервер |
| `axum` 0.8 | REST API + статика SPA |
| `tokio-rustls` 0.26 + `rustls` 0.23/0.24 | TLS MITM + upstream |
| `rcgen` 0.14 | генерация root CA и leaf-сертификатов |
| `moka` 0.12 | in-memory кеш + single-flight (`get_with`) |
| `dashmap` 6.x | sharded map (leaf certs, inflight) |
| `serde` + `toml` + `serde_json` | конфиг и metadata |
| `blake3` + `hex` | cache key (hex-путь — нет path traversal) |
| `ipnet` | CIDR-исключения |
| `clap` 4 | CLI |
| `rust-embed` | embed `web/dist` |
| `webpki-roots` | корни для upstream TLS |
| `notify` | hot-reload конфига |

SOCKS5 server-handshake (~80 строк) — **in-house**, без `tokio-socks`.

### Порты (по умолчанию)

| Порт | Служба |
|---|---|
| `3128` | HTTP proxy (кеширует http://, CONNECT — tunnel) |
| `3129` | SSL/HTTPS proxy (MITM + кеш https://) |
| `1080` | SOCKS5 (TCP tunnel, без кеша) |
| `8080` | Web UI + REST API (`127.0.0.1`) |

### Data flow

1. **HTTP proxy:** absolute-URI → cache key (blake3) → exclusions → mem/disk lookup → HIT / revalidate 304 / MISS+single-flight → stream в клиент и на диск.
2. **CONNECT (HTTP port):** `200 Connection Established` → `copy_bidirectional` (только туннель).
3. **HTTPS MITM:** CONNECT → rustls server с leaf-cert для host → парсинг HTTP → тот же cache path; upstream через rustls client + webpki-roots. Исключённые host — **splice** (raw tunnel, без MITM).
4. **SOCKS5:** handshake → CONNECT only → туннель. Кеш не применяется.

### Кеш на диске

```
cache/
├── index/ab/cd/<key>.json    # metadata (2-level hex fanout)
├── objects/ab/cd/<key>.body  # raw body
└── tmp/                      # atomic write → rename
```

- Key = hex blake3 → безопасные имена файлов.
- Eviction: LRU по `last_access` при превышении `cache.max_bytes`.
- Mem: moka с weigher = body len; disk — source of truth.

### CA / сертификаты

- Root CA (rcgen): `ca/ca.crt` (0644), `ca/ca.key` (**0600**), CN=`RustCache MITM Root`.
- Leaf: on-the-fly, SAN=host, RSA-2048, кеш в `DashMap`.
- Экспорт: CLI `export-ca` + `GET /api/ca.crt`.
- **Требование:** клиенты должны доверять этому CA, иначе HTTPS не кешируется.

### Статистика

- `AtomicU64`: hits, misses, bypasses, bytes_served, bytes_saved, errors, tunnels.
- Ring buffer 100: `ReqRecord { ts, method, url, host, status, outcome, duration_ms, resp_bytes }`.
- Исключения: exact / wildcard / suffix + CIDR, `ArcSwap` при reload.

### REST API

| Метод | Путь | Назначение |
|---|---|---|
| GET | `/api/health` | liveness |
| GET | `/api/stats` | счётчики, hit-rate, saved MB |
| GET | `/api/requests?limit=100` | последние запросы |
| GET | `/api/config` | эффективный конфиг |
| POST | `/api/config/reload` | hot-reload |
| GET/POST/DELETE | `/api/exclusions` | CRUD исключений |
| GET | `/api/ca.crt` | скачать root CA |
| GET/DELETE | `/api/cache` | размер / purge |
| GET | `/*` | SPA (embed-ui) |

---

## 3.1 Конфиг (TOML)

`config.example.toml`:
```toml
[http]
port = 3128
[https]
port = 3129          # MITM
[socks5]
port = 1080
[api]
bind = "127.0.0.1:8080"

[cache]
dir = "~/.local/share/rustcache/cache"
max_bytes = 2147483648   # 2 GiB
max_object_bytes = 52428800

[exclude]
domains = ["*.local", "bank.example", "exact.host"]
cidrs = ["192.168.0.0/16", "10.0.0.0/8"]

[ca]
dir = "~/.local/share/rustcache/ca"
```

Hot-reload через `notify` (ports — restart listeners; exclusions/cache/log — на лету).

## 3.2 rustcache-core

| Модуль | Что делает |
|---|---|
| `cache/key` | canonical URL + blake3 + hex path |
| `cache/disk` | objects/index/tmp, atomic write |
| `cache/mem` | moka, weigher, TTL из cache policy |
| `cache/evict` | LRU eviction, accounting map |
| `cache/coalesce` | single-flight на MISS |
| `http/cache_policy` | Cache-Control, Expires, ETag, Vary |
| `http/fetch` | hyper client к origin |
| `certs/ca` | gen/load root CA, export PEM |
| `certs/leaf` | on-the-fly leaf + DashMap cache |
| `stats/metrics` | AtomicU64 counters |
| `stats/ring` | VecDeque 100 ReqRecord |
| `excl/matchers` | exact/wildcard/suffix + IpNet |

## 3.3 HTTP proxy listener

1. Parse request-line; `CONNECT` → tunnel; иначе absolute-URI.
2. Exclusions → bypass.
3. Lookup mem→disk; fresh → HIT; stale+ETag → revalidate; MISS → fetch+store.
4. Учёт stats.

## 3.4 SSL/HTTPS MITM listener

1. `CONNECT host:443` → 200 → rustls server + leaf для host.
2. ALPN `http/1.1` (h2 — phase 2).
3. Excluded host → **splice** (raw tunnel, без MITM).
4. Парсинг HTTP → cache path (scheme https).
5. Upstream: rustls client + webpki-roots.
6. WebSocket/Upgrade → bypass cache, pipe.
7. Ошибки протокола → runtime denylist host (лог).

## 3.5 SOCKS5 listener

- Method: no-auth; CMD=CONNECT only (UDP/Bind — reject).
- Dial → reply → `copy_bidirectional`.
- Stats: tunnels, bytes, duration.

## 3.6 CA CLI

```
rustcache gen-ca        # создать root CA если нет
rustcache export-ca     # путь / PEM
rustcache purge         # очистить кеш
rustcache run           # старт всех listeners + API
```

## 3.7 REST API (axum)

Реализовать таблицу API. Секреты в `/api/config` редактировать. Default bind localhost.

## 3.8 CLI + logging

`clap` + `tracing-subscriber` (env-filter). Не логировать Authorization/Cookie.

## 3.9 Сквозной сценарий (ручная проверка)

```bash
cargo run -p rustcache -- gen-ca
cargo run -p rustcache -- run --config config.toml

# HTTP MISS → HIT
curl -x http://127.0.0.1:3128 http://example.com/
curl -x http://127.0.0.1:3128 http://example.com/   # HIT

# HTTPS MITM
curl --cacert ~/.local/share/rustcache/ca/ca.crt \
     -x http://127.0.0.1:3129 https://example.com/

# SOCKS5
curl --socks5 127.0.0.1:1080 https://example.com/

# API
curl http://127.0.0.1:8080/api/stats
```

## Порядок реализации

1. config + logging + CLI skeleton
2. cache key + disk + mem + eviction
3. HTTP proxy + cache policy + fetch
4. stats ring + metrics
5. exclusions
6. CONNECT tunnel
7. SOCKS5
8. CA + MITM + leaf cache
9. REST API + purge + hot-reload

---

## Exit criteria

- [ ] HTTP MISS→HIT, saved bytes растут
- [ ] HTTPS MITM HIT с установленным CA
- [ ] Исключения: host/CIDR не кешируются и не MITM'ятся
- [ ] SOCKS5 tunnel работает
- [ ] `/api/stats`, `/api/requests`, exclusions CRUD, `/api/ca.crt`, purge
- [ ] CA key 0600

## Следующий шаг

`docs/plans/04-tests.md`

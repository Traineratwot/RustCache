# План 4 — Написание тестов

**Проект:** RustCache — кеширующий proxy-сервер.
**Каталог:** `/home/kirill/PhpstormProjects/RustCache`
**Цель:** `cargo test` зелёный; unit + integration покрывают cache/exclusions/certs/policy и e2e proxy.

**Предыдущий:** `docs/plans/03-implementation.md`
**Следующий:** `docs/plans/05-web-ui.md`

**Предусловие:** реализация из плана 3 (proxy + кеш + CA + API) должна существовать.

---

## 4.1 Unit (rustcache-core)

| Модуль | Кейсы |
|---|---|
| `cache/key` | нормализация URL, default port, fragment, hex path |
| `cache/disk` | atomic write, meta+body, path safety |
| `cache/evict` | LRU порядок, size accounting, cap |
| `cache/mem` | weigher, TTL, get_with single-flight |
| `http/cache_policy` | max-age, no-store, private, Expires, ETag, Vary |
| `excl/matchers` | exact/suffix/wildcard/CIDR, негативные |
| `certs/leaf` | SAN=host, подпись CA, parse-back PEM |
| `stats/ring` | capacity 100, order, snapshot |

Размещение: `#[cfg(test)] mod tests` в модулях или `crates/rustcache-core/tests/`.

## 4.2 Integration (`tests/`)

Fixture: локальный origin (hyper/tiny_http) с изменяемым body + proxy на ephemeral ports.

Сценарии:
1. MISS → HIT (2-й запрос быстрее, body идентичен)
2. max-age expiry → revalidate 304 → HIT-revalidated
3. `no-store` → никогда не кешируется
4. Coalescing: N параллельных одинаковых URL → 1 origin fetch
5. Exclusion bypass (domain + CIDR)
6. CONNECT tunnel echo
7. MITM HTTPS HIT (клиент с нашим CA в RootCertStore)
8. Purge all / by key
9. Hot-reload exclusions
10. SOCKS5 CONNECT + reject UDP

## 4.3 Smoke-скрипт

`scripts/smoke.sh` — curl-сценарии:

```bash
#!/usr/bin/env bash
set -euo pipefail
BASE=http://127.0.0.1:8080
HTTP_PROXY=http://127.0.0.1:3128
HTTPS_PROXY=http://127.0.0.1:3129
CA=$HOME/.local/share/rustcache/ca/ca.crt

echo "=== health ==="
curl -fsS "$BASE/api/health"

echo "=== HTTP MISS then HIT ==="
curl -fsS -x $HTTP_PROXY http://example.com/ -o /dev/null -w "%{http_code} %{time_total}\n"
curl -fsS -x $HTTP_PROXY http://example.com/ -o /dev/null -w "%{http_code} %{time_total}\n"

echo "=== HTTPS MITM ==="
curl -fsS --cacert "$CA" -x $HTTPS_PROXY https://example.com/ -o /dev/null -w "%{http_code}\n"

echo "=== SOCKS5 ==="
curl -fsS --socks5 127.0.0.1:1080 https://example.com/ -o /dev/null -w "%{http_code}\n"

echo "=== stats ==="
curl -fsS "$BASE/api/stats"
```

## 4.4 Опционально perf

`criterion` benches: blake3 key, mem-cache get (не полный proxy).

## 4.5 Ручной чеклист (Squid-parity)

- Firefox с импортированным CA: HTTPS-сайты открываются, HIT в `/api/requests`
- Повторная загрузка большого файла — saved MB
- Банк/исключение — splice, без MITM

---

## Exit criteria

- [ ] `cargo test` green (unit + integration)
- [ ] Покрыты: cache policy, eviction, exclusions, certs, coalescing, MITM HIT
- [ ] `scripts/smoke.sh` проходит
- [ ] `cargo clippy -- -D warnings` без warnings

## Следующий шаг

`docs/plans/05-web-ui.md`

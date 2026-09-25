# План 5 — Web-интерфейс на React

**Проект:** RustCache — кеширующий proxy-сервер.
**Каталог:** `/home/kirill/PhpstormProjects/RustCache`
**Цель:** SPA (Dashboard / Requests / Exclusions / Cache / Settings / CA), embed в бинарник, dev через Vite proxy.

**Предыдущий:** `docs/plans/04-tests.md`
**Это финальный план.**

**Предусловие:** REST API из плана 3 работает на `127.0.0.1:8080`.

---

## REST API (контракт для UI)

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

## 5.1 Scaffold

```bash
cd web
npm create vite@latest . -- --template react-ts
npm i react-router-dom
npm i -D tailwindcss @tailwindcss/vite   # или CSS modules
npm run build                            # → web/dist
```

`vite.config.ts`:
```ts
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  server: { proxy: { "/api": "http://127.0.0.1:8080" } },
});
```

## 5.2 API client

`web/src/api/client.ts` — typed fetch wrappers:
`getStats`, `getRequests`, `getExclusions`, `addExclusion`, `deleteExclusion`, `getCache`, `purgeCache`, `reloadConfig`, `downloadCa`.

Пример:
```ts
export async function getStats(): Promise<Stats> {
  const r = await fetch("/api/stats");
  if (!r.ok) throw new Error(`stats ${r.status}`);
  return r.json();
}
```

## 5.3 Страницы

| Маршрут | Содержимое |
|---|---|
| `/` | Dashboard: hit rate %, hits/misses, saved MB, tunnels, uptime |
| `/requests` | таблица 100 запросов, poll 2s, фильтр, badge HIT/MISS/BYPASS |
| `/exclusions` | CRUD domain/CIDR, пояснение «не кешируется и не MITM» |
| `/cache` | size vs cap, entries, purge all/host, confirm |
| `/settings` | ports/dirs/log level (read-only), кнопка Reload config |
| `/ca` | объяснение MITM-trust (RU), скачать `ca.crt`, SHA-256 fingerprint |

Навигация: `react-router-dom`, простой sidebar/header.

## 5.4 Embed в Rust

- Feature `embed-ui` + `rust-embed` на `web/dist`.
- Axum fallback: SPA static (index.html + assets), API под `/api`.
- Прод-сборка: `npm run build && cargo build --release --features embed-ui`.

В `crates/rustcache/src/ui_embed.rs` (feature `embed-ui`):
```rust
#[derive(rust_embed::Embed)]
#[folder = "../web/dist/"]
struct Assets;
```

## 5.5 Dev workflow

```bash
# терминал 1
cargo run -p rustcache -- run --config config.toml
# терминал 2
cd web && npm run dev    # Vite :5173 → proxy /api
```

## 5.6 Стилизация

Tailwind (быстро) или CSS modules. Без эмодзи в UI. **Язык интерфейса — русский.**

---

## Exit criteria

- [ ] Все 6 страниц работают против live API
- [ ] `npm run build` + `cargo build --features embed-ui` → UI открывается с `:8080`
- [ ] Dev: Vite proxy без CORS
- [ ] CA download + fingerprint
- [ ] Exclusions CRUD из UI
- [ ] Stats обновляются (poll)

---

## Verification (сводно по всем планам)

| План | Команда / проверка |
|---|---|
| 1 | `cargo run` hello; `cargo clippy` |
| 2 | `mimo mcp`; `/rustcache-dev` |
| 3 | curl MISS→HIT; MITM с CA; `/api/stats` |
| 4 | `cargo test`; `scripts/smoke.sh` |
| 5 | UI Dashboard/Requests; embed build |

## Security (нормативно)

- CA key `0600`, каталог `0700`; world-readable — warn/refuse.
- Не логировать `Authorization`/`Cookie`/секреты query.
- Cache keys только hex — нет path traversal.
- API default `127.0.0.1`.
- MITM — явное действие оператора; README + UI banner.

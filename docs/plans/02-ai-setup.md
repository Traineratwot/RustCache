# План 2 — Настройка AI (MCP, skills)

**Проект:** RustCache — кеширующий proxy-сервер.
**Каталог:** `/home/kirill/PhpstormProjects/RustCache`
**Цель:** чтобы агент эффективно работал над RustCache: контекст проекта, MCP-доки, проектные skills.

**Предыдущий:** `docs/plans/01-project-setup.md`
**Следующий:** `docs/plans/03-implementation.md`

---

## 2.1 AGENTS.md / инструкции проекта

Создать `/home/kirill/PhpstormProjects/RustCache/AGENTS.md`:

```markdown
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
```

## 2.2 Проектный конфиг MiMoCode

Создать `.mimocode/mimocode.jsonc` (merge поверх `~/.config/mimocode/mimocode.jsonc`):

```jsonc
{
  "$schema": "https://mimo.xiaomi.com/mimocode/config.json",
  "mcp": {
    "context7": {
      "type": "local",
      "command": ["npx", "-y", "@upstash/context7-mcp"],
      "enabled": true
    }
  },
  "formatter": {
    "rust": { "command": "rustfmt" }
  }
}
```

**Зачем context7:** актуальные доки tokio/hyper/rustls/axum/rcgen/moka.

Проверка:
```bash
mimo mcp
# в TUI: /mcps
```

## 2.3 Проектные skills

### `.mimocode/skills/rustcache-dev/SKILL.md`

```markdown
---
name: rustcache-dev
description: "Development workflow for RustCache caching proxy (Rust workspace + React UI). Use when user asks to build, run, add a feature, or debug rustcache / caching proxy. Triggers: 'сбилди rustcache', 'запусти proxy', 'добавь фичу в rustcache', 'build rustcache'."
---

# RustCache dev workflow

1. Read AGENTS.md for stack, ports, rules.
2. cargo fmt && cargo clippy -- -D warnings && cargo test
3. cargo run -p rustcache -- run --config config.toml
4. Modules: crates/rustcache-core (cache/certs/stats/excl), crates/rustcache (listeners/api/config)
5. UI: npm --prefix web run dev (Vite proxy → :8080)
```

### `.mimocode/skills/proxy-smoke-test/SKILL.md`

```markdown
---
name: proxy-smoke-test
description: "Smoke-test RustCache proxy: HTTP HIT/MISS, HTTPS MITM with CA, SOCKS5, API stats. Use when user asks to check proxy, run smoke test, or curl through proxy. Triggers: 'проверь proxy', 'smoke test rustcache', 'curl через proxy'."
---

# RustCache smoke test

1. cargo run -p rustcache -- gen-ca
2. cargo run -p rustcache -- run --config config.toml &
3. curl -x http://127.0.0.1:3128 http://example.com/  twice (MISS then HIT)
4. curl --cacert ~/.local/share/rustcache/ca/ca.crt -x http://127.0.0.1:3129 https://example.com/
5. curl --socks5 127.0.0.1:1080 https://example.com/
6. curl http://127.0.0.1:8080/api/stats
Expect: HIT/MISS, saved bytes grow, CA key 0600
```

Формат skill: `name` kebab-case, `description` — что делает + когда (фразы-триггеры).

## 2.4 Опционально (позже)

- `permission.external_directory` для путей кеша/CA
- custom agent `.mimocode/agent/proxy-tester.md`
- remote skill index в `skills.urls[]`

---

## Exit criteria

- [ ] `AGENTS.md` описывает stack, команды, security-правила
- [ ] `.mimocode/mimocode.jsonc` с MCP context7, `mimo mcp` видит сервер
- [ ] skills `rustcache-dev` и `proxy-smoke-test` установлены, вызываются через `/`
- [ ] smoke: агент по `/rustcache-dev` понимает workflow

## Следующий шаг

`docs/plans/03-implementation.md`

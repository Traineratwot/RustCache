# План 1 — Настройка проекта (rust, git, hello world)

**Проект:** RustCache — кеширующий proxy-сервер.
**Каталог:** `/home/kirill/PhpstormProjects/RustCache`
**Цель:** рабочий toolchain + git-репозиторий + cargo workspace, который собирается и стартует.

**Запускать первым.** Следующий: `docs/plans/02-ai-setup.md`

---

## 1.1 Проверка toolchain

| Инструмент | Ожидаемая версия | Путь |
|---|---|---|
| rustc / cargo | 1.98.x | `~/.cargo/bin` |
| git | 2.5x | `/usr/bin/git` |
| node / npm | 22.x | `~/.bun/bin` |

```bash
rustc --version
cargo --version
git --version
node --version
npm --version
rustup component add rustfmt clippy
```

Если rust отсутствует:
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup default stable
rustup component add rustfmt clippy
```

## 1.2 Git-репозиторий

```bash
cd /home/kirill/PhpstormProjects/RustCache
git init -b main
```

Создать `.gitignore`:
```
/target
**/*.rs.bk
web/node_modules
web/dist
.env
*.key
ca/
cache/
config.toml
```

## 1.3 Cargo workspace + hello world

Структура:
```
RustCache/
├── Cargo.toml                  # [workspace]
├── crates/
│   ├── rustcache/              # bin
│   └── rustcache-core/         # lib
```

`Cargo.toml` (workspace root):
```toml
[workspace]
resolver = "2"
members = ["crates/rustcache", "crates/rustcache-core"]

[workspace.package]
version = "0.1.0"
edition = "2021"
license = "MIT"
```

```bash
cargo new crates/rustcache --bin --vcs none
cargo new crates/rustcache-core --lib --vcs none
```

Связать members с workspace (в `crates/*/Cargo.toml`):
```toml
[package]
name = "rustcache"   # или rustcache-core
version.workspace = true
edition.workspace = true
```

Минимальный `crates/rustcache/src/main.rs`:
```rust
fn main() {
    println!("RustCache v{} — caching proxy", env!("CARGO_PKG_VERSION"));
}
```

Минимальный `crates/rustcache-core/src/lib.rs`:
```rust
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
```

## 1.4 Сборка и smoke-check

```bash
cargo build
cargo run -p rustcache
# ожидаемый вывод: "RustCache v0.1.0 — caching proxy"
cargo fmt --check
cargo clippy -- -D warnings
```

## 1.5 Базовый README

Создать `README.md`:
- что за проект (кеш proxy HTTP/HTTPS, SOCKS5, Web UI);
- порты по умолчанию: 3128 HTTP, 3129 HTTPS-MITM, 1080 SOCKS5, 8080 API/UI;
- как собрать: `cargo build --release`;
- ссылка на `docs/plans/`.

---

## Exit criteria

- [ ] `git init` выполнен, `.gitignore` на месте
- [ ] `cargo build` без ошибок
- [ ] `cargo run -p rustcache` печатает hello
- [ ] `cargo clippy -- -D warnings` чистый
- [ ] `README.md` создан

## Следующий шаг

`docs/plans/02-ai-setup.md`

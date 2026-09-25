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

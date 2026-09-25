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

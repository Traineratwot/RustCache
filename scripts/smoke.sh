#!/usr/bin/env bash
set -euo pipefail

BASE=${BASE:-http://127.0.0.1:8080}
HTTP_PROXY=${HTTP_PROXY:-http://127.0.0.1:3128}
HTTPS_PROXY=${HTTPS_PROXY:-http://127.0.0.1:3129}
SOCKS5=${SOCKS5:-127.0.0.1:1080}
CA=${CA:-$HOME/.local/share/rustcache/ca/ca.crt}

echo "=== health ==="
curl -fsS "$BASE/api/health"
echo

echo "=== HTTP MISS then HIT ==="
curl -fsS -x "$HTTP_PROXY" http://example.com/ -o /dev/null -w "%{http_code} %{time_total}\n"
curl -fsS -x "$HTTP_PROXY" http://example.com/ -o /dev/null -w "%{http_code} %{time_total}\n"

echo "=== HTTPS MITM ==="
curl -fsS --cacert "$CA" -x "$HTTPS_PROXY" https://example.com/ -o /dev/null -w "%{http_code}\n"

echo "=== SOCKS5 ==="
curl -fsS --socks5 "$SOCKS5" https://example.com/ -o /dev/null -w "%{http_code}\n"

echo "=== stats ==="
curl -fsS "$BASE/api/stats"
echo
echo "smoke ok"

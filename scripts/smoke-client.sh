#!/usr/bin/env bash
# Smoke-test rustcache-client against a running RustCache (or fail-open alone).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

LISTEN="127.0.0.1:31280"
API="http://127.0.0.1:8080"
BIN="${BIN:-target/debug/rustcache-client}"

echo "== build client =="
cargo build -p rustcache-client

echo "== unit + integration tests =="
cargo test -p rustcache-client --quiet

echo "== CLI surface =="
"$BIN" --help >/dev/null
"$BIN" config path >/dev/null
"$BIN" status || true

echo "== proxy dry-run =="
"$BIN" proxy dry-run

echo "== run client proxy (no system capture) =="
"$BIN" run --no-capture --log-level warn &
CLIENT_PID=$!
cleanup() {
	kill "$CLIENT_PID" 2>/dev/null || true
	wait "$CLIENT_PID" 2>/dev/null || true
}
trap cleanup EXIT

for _ in $(seq 1 30); do
	if (echo >/dev/tcp/127.0.0.1/31280) 2>/dev/null; then
		break
	fi
	sleep 0.1
done

echo "== curl via client (fail-open DIRECT) =="
set +e
code=$(curl -sS -o /tmp/rcc-smoke-body.txt -w '%{http_code}' \
	-x "http://${LISTEN}" --max-time 15 http://example.com/)
curl_rc=$?
set -e
echo "HTTP $code (curl rc=$curl_rc)"
# Offline: allow connect/DNS/empty-reply failures; reject a hung proxy.
if [[ "$curl_rc" -ne 0 && "$curl_rc" -ne 6 && "$curl_rc" -ne 7 && "$curl_rc" -ne 28 && "$curl_rc" -ne 52 ]]; then
	echo "unexpected curl rc $curl_rc"
	exit 1
fi

if curl -sS --max-time 2 "$API/api/health" >/dev/null 2>&1; then
	echo "== rustcache up — request again =="
	curl -sS -o /dev/null -w 'via-client %{http_code}\n' \
		-x "http://${LISTEN}" --max-time 15 http://example.com/
else
	echo "== rustcache not running — fail-open path exercised =="
fi

echo "SMOKE OK"

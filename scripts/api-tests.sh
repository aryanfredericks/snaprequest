#!/usr/bin/env bash
# Runs the example collection and flow (examples/shop) against the mock shop API, like the
# `api-tests` CI job. Reports go to target/api-reports/.
set -euo pipefail
cd "$(dirname "$0")/.."

PORT="${MOCK_PORT:-8765}"
REPORTS=target/api-reports
export SNAPREQ_SECRET_PASSWORD="${SNAPREQ_SECRET_PASSWORD:-letmein}"
# Keep OAuth caches and the like out of the user's directories.
export SNAPREQ_CACHE_DIR="${SNAPREQ_CACHE_DIR:-target/api-cache}"

cargo build --quiet -p snapreq-cli -p snapreq-test --example mock_shop
SNAPREQ=target/debug/snapreq
target/debug/examples/mock_shop "$PORT" &
MOCK=$!
trap 'kill $MOCK 2>/dev/null || true' EXIT
for _ in $(seq 50); do
  if "$SNAPREQ" send "http://127.0.0.1:$PORT/health" -t 1 >/dev/null 2>&1; then break; fi
  sleep 0.1
done

rm -rf "$REPORTS"
status=0
echo "==> collection"
"$SNAPREQ" run examples/shop -e ci --var "base_url=http://127.0.0.1:$PORT" \
  --report "junit:$REPORTS/collection.xml" --report "html:$REPORTS/collection.html" || status=$?
echo "==> checkout flow"
"$SNAPREQ" run examples/shop/orders/checkout.flow.toml -e ci --var "base_url=http://127.0.0.1:$PORT" \
  --report "junit:$REPORTS/checkout.xml" --report "html:$REPORTS/checkout.html" || status=$?
exit "$status"

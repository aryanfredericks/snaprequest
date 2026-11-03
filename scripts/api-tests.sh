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

# Two builds: a target flag such as --example limits a build to that target.
cargo build --quiet -p snapreq-cli
cargo build --quiet -p snapreq-test --example mock_shop
SNAPREQ=target/debug/snapreq
for bin in "$SNAPREQ" target/debug/examples/mock_shop; do
  [ -x "$bin" ] || { echo "error: $bin was not built" >&2; exit 1; }
done
target/debug/examples/mock_shop "$PORT" &
MOCK=$!
trap 'kill $MOCK 2>/dev/null || true' EXIT
ready=0
for _ in $(seq 50); do
  if "$SNAPREQ" send "http://127.0.0.1:$PORT/health" -t 1 >/dev/null 2>&1; then ready=1; break; fi
  sleep 0.1
done
[ "$ready" = 1 ] || { echo "error: the mock API did not start on port $PORT" >&2; exit 1; }

rm -rf "$REPORTS"
status=0
echo "==> collection"
"$SNAPREQ" run examples/shop -e ci --var "base_url=http://127.0.0.1:$PORT" \
  --report "junit:$REPORTS/collection.xml" --report "html:$REPORTS/collection.html" || status=$?
echo "==> checkout flow"
"$SNAPREQ" run examples/shop/orders/checkout.flow.toml -e ci --var "base_url=http://127.0.0.1:$PORT" \
  --report "junit:$REPORTS/checkout.xml" --report "html:$REPORTS/checkout.html" || status=$?
exit "$status"

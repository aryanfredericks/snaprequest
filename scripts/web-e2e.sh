#!/usr/bin/env bash
# End-to-end tests of the web UI (web/e2e, Playwright): builds the UI and the server, starts the
# mock shop API and `snapreq serve` on a copy of examples/shop, then drives a headless Chromium.
# Usage: scripts/web-e2e.sh [playwright args]   (needs `npx playwright install chromium` once)
set -euo pipefail
cd "$(dirname "$0")/.."

MOCK_PORT=8765 # examples/shop's `ci` environment points here
PORT="${SNAPREQ_E2E_PORT:-7881}"
DIR=target/web-e2e
export SNAPREQ_SECRET_PASSWORD=letmein
export SNAPREQ_CACHE_DIR="$DIR/cache"

(cd web && npm run build >/dev/null)
cargo build --quiet -p snapreq-cli
cargo build --quiet -p snapreq-test --example mock_shop
SNAPREQ=target/debug/snapreq

rm -rf "$DIR"
mkdir -p "$DIR/workspace"
cp -r examples/shop "$DIR/workspace/shop"

target/debug/examples/mock_shop "$MOCK_PORT" &
MOCK=$!
"$SNAPREQ" serve --bind "127.0.0.1:$PORT" --data-dir "$DIR/data" --workspace "$DIR/workspace" >"$DIR/server.log" 2>&1 &
SERVER=$!
trap 'kill $MOCK $SERVER 2>/dev/null || true' EXIT
for url in "http://127.0.0.1:$MOCK_PORT/health" "http://127.0.0.1:$PORT/healthz"; do
  ready=0
  for _ in $(seq 100); do
    if "$SNAPREQ" send "$url" -t 1 >/dev/null 2>&1; then ready=1; break; fi
    sleep 0.1
  done
  [ "$ready" = 1 ] || { echo "error: $url did not come up" >&2; cat "$DIR/server.log" >&2; exit 1; }
done

cd web
SNAPREQ_E2E_URL="http://127.0.0.1:$PORT" SNAPREQ_E2E_MOCK="http://127.0.0.1:$MOCK_PORT" \
  SNAPREQ_E2E_WORKSPACE="$(cd ../$DIR/workspace && pwd)" npx playwright test "$@"

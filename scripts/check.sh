#!/usr/bin/env bash
# Runs the same checks as CI. Usage: scripts/check.sh
set -euo pipefail
cd "$(dirname "$0")/.."

msrv=$(sed -n 's/^rust-version = "\(.*\)"/\1/p' Cargo.toml)

step() { printf '\n==> %s\n' "$*"; }

step "format";  cargo fmt --all --check
step "clippy";  cargo clippy --workspace --all-targets -- -D warnings
step "tests";   cargo test --workspace
step "api tests (examples/shop)"; scripts/api-tests.sh
if command -v npm >/dev/null; then
  step "web UI"
  (cd web && npm ci --no-audit --no-fund >/dev/null \
    && cp src/lib/api/schema.d.ts ../target/schema.d.ts.before && npm run gen:api >/dev/null \
    && { cmp -s src/lib/api/schema.d.ts ../target/schema.d.ts.before \
         || { echo "error: src/lib/api/schema.d.ts was out of date (now regenerated)" >&2; exit 1; }; } \
    && npm run check && npm test && npm run build)
  if ls "${PLAYWRIGHT_BROWSERS_PATH:-$HOME/.cache/ms-playwright}"/chromium_headless_shell-* >/dev/null 2>&1; then
    step "web UI end-to-end"; scripts/web-e2e.sh
  else
    echo "skipping the web UI end-to-end tests (install with: cd web && npx playwright install chromium)"
  fi
else
  echo "skipping the web UI (needs Node.js 24 and npm)"
fi
if command -v cargo-deny >/dev/null; then
  step "cargo-deny"; cargo deny check
else
  echo "skipping cargo-deny (install with: cargo install cargo-deny --locked)"
fi
if rustup toolchain list | grep -q "^$msrv"; then
  step "MSRV $msrv"; cargo "+$msrv" check --workspace --all-targets
else
  echo "skipping MSRV check (install with: rustup toolchain install $msrv --profile minimal)"
fi
printf '\nAll checks passed.\n'

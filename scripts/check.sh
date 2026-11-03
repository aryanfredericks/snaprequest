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

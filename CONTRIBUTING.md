# Contributing to SnapRequest

Thanks for your interest in contributing!

## Getting started

1. Install a stable Rust toolchain with [rustup](https://rustup.rs). The repository's
   `rust-toolchain.toml` adds `rustfmt` and `clippy` automatically.
2. Build and test:

   ```sh
   cargo build --workspace
   cargo test --workspace
   ```

3. Optional, for the web UI (`web/`, Svelte 5 + TypeScript + Vite): Node.js 24 and npm.

   ```sh
   cd web
   npm ci
   npm run dev      # UI on http://localhost:5173, API forwarded to `snapreq serve` on :7878
   npm run build    # web/dist, embedded into release builds of snapreq
   npm test         # unit tests (Vitest)
   npx playwright install chromium   # once
   npm run e2e      # end-to-end tests in a headless browser (scripts/web-e2e.sh)
   ```

   Without a build, `snapreq serve` shows a page explaining how to build the UI. When an API handler changes,
   refresh the typed client:

   ```sh
   SNAPREQ_UPDATE_OPENAPI=1 cargo test -p snapreq-server --test openapi   # rewrites web/openapi.json
   npm --prefix web run gen:api                                           # rewrites src/lib/api/schema.d.ts
   ```

4. Read [`docs/PLAN.md`](docs/PLAN.md) for the architecture and roadmap, and
   [`docs/DECISIONS.md`](docs/DECISIONS.md) for the design decisions behind it.

## Before opening a pull request

Run `scripts/check.sh`. It runs the same checks as CI:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check                                  # licenses and security advisories
cargo +1.89 check --workspace --all-targets       # minimum supported Rust version
```

It also runs the web UI checks (`npm run check`, `npm test`, `npm run build`, generated types up to date).
Those, cargo-deny and the MSRV build are skipped with a hint if npm, `cargo-deny` or the MSRV toolchain is not
installed.

## Snapshot tests

CLI output is covered by [`insta`](https://insta.rs) snapshots in `crates/snapreq-cli/src/snapshots/`.
When you change output on purpose, update and review them:

```sh
cargo install cargo-insta --locked   # once
cargo insta test -p snapreq-cli --review
```

Snapshots render fixed sample data (`src/fixtures.rs`), so they never depend on ports, dates or timing.

## Guidelines

- **Tests ship with features.** New behaviour needs tests; bug fixes need a regression test.
- **Keep `snapreq-core` independent** of UI, CLI and storage code.
- **Errors:** libraries use `thiserror`, binaries use `anyhow`. Use `tracing` for logs, never `println!` in libraries.
- **No `unsafe`.** The workspace forbids it.
- **New dependencies** go into `[workspace.dependencies]` in the root `Cargo.toml` and must have an
  MIT-compatible license (checked by `cargo deny`).
- **Architectural changes** need an entry in `docs/DECISIONS.md`.
- Keep pull requests focused; one logical change per PR.

## Reporting bugs and security issues

Open a GitHub issue for bugs. For security vulnerabilities, please do **not** open a public issue;
use GitHub's private vulnerability reporting for this repository instead.

By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE).

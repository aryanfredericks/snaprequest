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

3. Read [`docs/PLAN.md`](docs/PLAN.md) for the architecture and roadmap, and
   [`docs/DECISIONS.md`](docs/DECISIONS.md) for the design decisions behind it.

## Before opening a pull request

Run `scripts/check.sh`. It runs the same checks as CI:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check                                  # licenses and security advisories
cargo +1.88 check --workspace --all-targets       # minimum supported Rust version
```

The last two are skipped with a hint if `cargo-deny` or the MSRV toolchain is not installed.

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

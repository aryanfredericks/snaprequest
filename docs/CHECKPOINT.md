# Checkpoint — where we are

> Read this first when resuming a session. Update it at the end of every session.

## Current position
- **Phase:** 0 Foundation, nearly complete
- **Current checkpoint:** P0.3: CI is written but has not run yet (no GitHub remote)
- **Next checkpoint after that:** P1.1, the request/response data model in `snapreq-core`

## Waiting on the maintainer
| # | Item | Notes |
|---|---|---|
| 1 | First commit and push to `origin` | Remote `origin` = https://github.com/aryanfredericks/snaprequest.git (added, repo empty). Pushing runs CI for the first time, which closes P0.3. |
| 2 | Code of conduct contact | `CODE_OF_CONDUCT.md` points to GitHub private reporting; add a contact email if wanted. |

## Decisions (all accepted 2026-10-31, see DECISIONS.md)
- Name **SnapRequest**, binary `snapreq`, crates `snapreq-*` (ADR-009)
- License **MIT** (ADR-007)
- Web UI **Svelte 5 + TypeScript** (ADR-005)
- Collections in **TOML**, one file per request (ADR-004)

Local directory: `/home/cruxi/Projects/snap-request` (renamed from `thunder-http` on 2026-10-31).

## Environment (verified 2026-10-31)
- rustc / cargo 1.99.0, Node v24.18.0, git 2.56.0, Linux (Fedora)
- `cargo-deny` is **not** installed locally (`cargo install cargo-deny --locked` to run it); the CI job runs it.
- MSRV is set to 1.85 (minimum for edition 2024); only checked in CI, not locally.

## Next steps
1. Commit and push (item 1 above). Confirm CI is green, then mark P0.3 done.
2. P1.1: define `Request`, `Response`, `Method`, `HeaderMap` usage, `Body` variants, `RequestSettings`,
   `Timings` and `TraceEvent` in `snapreq-core`, with serde round-trip tests.
   Add `http`, `url`, `bytes`, `serde` to the workspace dependencies.

## Progress
| Phase | Status |
|---|---|
| 0 Foundation | 🟨 in progress (P0.3 waiting on first CI run) |
| 1 Core HTTP engine | ⬜ |
| 2 CLI MVP | ⬜ |
| 3 Collections & environments | ⬜ |
| 4 Auth helpers | ⬜ |
| 5 Assertions & runner | ⬜ |
| 6 Self-hosted server | ⬜ |
| 7 Web UI | ⬜ |
| 8 Multi-user & hardening | ⬜ |
| 9 Interop & scripting | ⬜ |
| 10 Advanced protocols | ⬜ |
| 11 Release 1.0 | ⬜ |

Legend: ⬜ not started · 🟨 in progress · ✅ done

## Completed checkpoints
- P0.1 — 2026-10-31 — `git init` (branch `main`), Cargo workspace (resolver 3, edition 2024) with `snapreq-core`, `-collection`, `-test`, `-server`, `-cli`; `rust-toolchain.toml`, `rustfmt.toml`, `.gitignore`, `.editorconfig`.
- P0.2 — 2026-10-31 — `LICENSE` (MIT), `README.md`, `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`.
- P0.4 — 2026-10-31 — Shared dependency versions in `[workspace.dependencies]`; workspace lints (`unsafe_code = forbid`, clippy warnings); `thiserror` error enum and `Result` alias per library crate; CLI uses `anyhow` + `tracing` (level via `--log-level` or `SNAPREQ_LOG`); release profile (thin LTO, strip).

P0.3 (written, not yet verified): `.github/workflows/ci.yml` (fmt, clippy, tests on Linux/macOS/Windows, MSRV, cargo-deny) and `deny.toml`.

## Session log
- **2026-10-31 (1):** Planning. Wrote PLAN.md, DECISIONS.md, CHECKPOINT.md, CLAUDE.md.
- **2026-10-31 (2):** Decisions settled (SnapRequest / MIT / Svelte+TS / TOML). Renamed docs. Did P0.1, P0.2, P0.4 and wrote P0.3. `cargo fmt`, `clippy -D warnings` and `cargo test` all pass; `snapreq --version` prints `snapreq 0.1.0`.
- **2026-10-31 (3):** `LICENSE` and `authors` set to Aryan Fredericks; repo URLs in `Cargo.toml` point to github.com/aryanfredericks/snaprequest; added `origin` remote; renamed the local folder to `snap-request`.

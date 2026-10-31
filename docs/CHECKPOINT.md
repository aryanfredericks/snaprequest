# Checkpoint — where we are

> Read this first when resuming a session. Update it at the end of every session.

## Current position
- **Phase:** 1 Core HTTP engine, in progress
- **Current checkpoint:** P1.4, instrumented connector

## Waiting on the maintainer
| # | Item | Notes |
|---|---|---|
| 1 | Code of conduct contact | `CODE_OF_CONDUCT.md` points to GitHub private reporting; add a contact email if wanted. |

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
1. P1.4: instrumented connector. Replace `tokio::net::lookup_host` with `hickory-resolver` (timed DNS,
   per-request host overrides like curl `--resolve`), Happy Eyeballs (RFC 8305) for IPv6/IPv4,
   and the TLS step with `rustls`/`tokio-rustls` emitting `TlsStart`/`TlsEnd` (version, cipher, ALPN,
   certificate chain via `x509-parser`). Remove the `Error::Unsupported("HTTPS")` guard in `Client::execute`.
   TLS *options* (custom CA, mTLS, verify off, min/max version) stay in P1.5.
   Integration tests need a local TLS server with a self-signed cert generated in the test (`rcgen`).

## Body notes (P1.3)
- `client/body.rs`: a body is encoded once into segments (bytes or file ranges) with an exact length; `to_body()` builds a fresh streaming body each time (retries reopen files). Files stream in 64 KiB chunks.
- `Content-Length` is always exact (file sizes read at prepare time). If a file shrinks before sending, the request fails with "shrank while being sent"; if it grows, only the announced length is sent.
- Multipart: random boundary `----snapreq-…`; names/filenames escaped per WHATWG; part content type guessed from extension (`mime_guess`). A user `Content-Type: multipart/form-data` without boundary gets the boundary appended.
- File paths are used as given (relative to the process CWD). Collections (P3) must resolve paths relative to the collection directory before calling core.
- Response bodies are still fully buffered in memory. Streaming responses to disk and a max-size limit are left for P2.5 (`-o file`).

## Engine notes (P1.2)
- `crates/snapreq-core/src/client/`: `mod.rs` (Client, execute, exchange, retry), `conn.rs` (connect + pool), `meter.rs` (MeteredIo socket wrapper), `recorder.rs` (trace + failure), `build.rs` (model → http::Request).
- Own pool per ADR-010. Pool key = scheme/host/port. Idle timeout 90 s, max 8 idle per host.
- A pooled connection that fails before any response byte is retried once on a new connection (trace shows an `info` event).
- DNS uses `tokio::net::lookup_host` for now and tries addresses sequentially; hickory + Happy Eyeballs + `--resolve` overrides belong to P1.4.
- HTTPS returns `Error::Unsupported` until TLS lands (P1.4/P1.5).
- Request headers go out title-cased (hyper `title_case_headers`); response header names are lowercase (hyper normalizes). Exact bytes come with wire capture (P1.8).
- `Sizes.response_headers` = bytes read − body bytes, so it includes chunked framing overhead until P1.8.
- Example: `cargo run -p snapreq-core --example get -- http://example.com/`.

## Model notes (P1.1, for later phases)
- Model lives in `crates/snapreq-core/src/model/`: `Request`, `Body`, `KeyValue`, `RequestSettings`, `Response`, `Timings`, `Sizes`, `TraceEntry`/`TraceEvent`, `TlsInfo`, `CertificateInfo`.
- Headers and query params are ordered `Vec<KeyValue>` with an `enabled` flag (duplicates allowed), not maps.
- JSON conventions: durations are `*_ms` floats with microsecond precision; enums use snake_case `type`/`event`/`kind` tags; response body is `{encoding: utf8|base64, data}`.
- `Timings::from_trace` derives timings from the trace (final hop only; `total` = last event). The engine must never set timings separately.
- `Request::effective_url` only accepts http/https with a host.

## Progress
| Phase | Status |
|---|---|
| 0 Foundation | ✅ done |
| 1 Core HTTP engine | 🟨 in progress (P1.1–P1.3 done) |
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
- P0.3 — 2026-10-31 — GitHub Actions CI (fmt, clippy, tests on Linux/macOS/Windows, MSRV 1.85, cargo-deny). First run all green.
- P1.1 — 2026-10-31 — Request/response/trace data model with serde, 22 unit tests.
- P1.2 — 2026-10-31 — `Client::execute` over HTTP/1.1 with own connection pool, socket metering, timeouts, stale-connection retry. 47 tests total (16 integration tests against local servers).
- P1.3 — 2026-10-31 — Streaming request bodies: text, JSON, form, file, multipart with file uploads; exact Content-Length. 58 tests total.

## Session log
- **2026-10-31 (1):** Planning. Wrote PLAN.md, DECISIONS.md, CHECKPOINT.md, CLAUDE.md.
- **2026-10-31 (2):** Decisions settled (SnapRequest / MIT / Svelte+TS / TOML). Renamed docs. Did P0.1, P0.2, P0.4 and wrote P0.3. `cargo fmt`, `clippy -D warnings` and `cargo test` all pass; `snapreq --version` prints `snapreq 0.1.0`.
- **2026-10-31 (3):** `LICENSE` and `authors` set to Aryan Fredericks; repo URLs in `Cargo.toml` point to github.com/aryanfredericks/snaprequest; added `origin` remote; renamed the local folder to `snap-request`.
- **2026-10-31 (4):** First commit pushed to GitHub (authored by Aryan Fredericks only; no Claude attribution). CI all green, Phase 0 done. P1.1 done: data model in `snapreq-core` (added `http`, `url`, `bytes`, `base64` deps; `clippy.toml` allows unwrap in tests).
- **2026-10-31 (5):** P1.1 committed and pushed (CI green). P1.2 done: HTTP/1.1 engine (ADR-010: own pool on hyper's low-level conn API). Verified against http://example.com.
- **2026-10-31 (6):** P1.2 committed and pushed (CI green). P1.3 done: streaming bodies (added `futures-util`, `tokio-util`, `mime_guess`).

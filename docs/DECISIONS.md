# Architecture Decision Log

Each entry: context, decision, consequences. Status is one of: **Proposed**, **Accepted**, **Superseded**.

---

## ADR-001: Rust for the whole backend and CLI
**Status:** Accepted
**Context:** The goal is a fast, low-memory tool that ships as a single binary.
**Decision:** Rust (stable toolchain, edition 2024), Cargo workspace.
**Consequences:** Single static binary per platform; strong typing for the file format and API.

## ADR-002: hyper instead of reqwest for the HTTP engine
**Status:** Accepted
**Context:** The headline feature is detailed logs: separate DNS, TCP, TLS, TTFB and download timings,
TLS handshake details and raw wire bytes. `reqwest` hides the connection phases.
**Decision:** Build on `hyper` 1.x + `hyper-util` with our own instrumented connector
(hickory DNS → tokio TCP → rustls TLS), emitting `TraceEvent`s at every step.
**Consequences:** More code to own (redirects, cookies, proxies, decompression), but full visibility and control.

## ADR-003: rustls for TLS
**Status:** Accepted
**Context:** We need handshake details (version, cipher, ALPN, certificate chain) and cross-platform builds without OpenSSL.
**Decision:** `rustls` + `tokio-rustls`, with `webpki-roots` and `rustls-native-certs`.
**Consequences:** No OpenSSL dependency. Some legacy servers (very old TLS versions or ciphers) will not be reachable; this is acceptable.

## ADR-004: TOML, one file per request, for collections
**Status:** Accepted (2026-10-31)
**Context:** Collections should be git-friendly: small diffs, easy review, few merge conflicts.
**Decision:** A collection is a directory. `snapreq.toml` at the root, one `<name>.toml` per request, folders as subdirectories, environments in `environments/`.
TOML is chosen over YAML because the Rust YAML ecosystem is unsettled and TOML has no indentation pitfalls; multi-line bodies use `'''` strings.
**Consequences:** Large JSON bodies can optionally live in a sibling file (`body_file = "create-user.json"`).

## ADR-005: Svelte + TypeScript for the web UI, embedded in the binary
**Status:** Accepted (2026-10-31)
**Context:** The UI needs rich editors (CodeMirror), virtualized lists and fast iteration. A Rust/WASM framework (Leptos, Dioxus) would keep everything in Rust, but its editor and component ecosystem is less mature.
**Decision:** Svelte 5 + TypeScript + Vite, built to static files and embedded with `rust-embed`, so deployment stays a single binary.
**Consequences:** Contributors need Node.js only to work on the UI; end users never do.
**Alternative:** Leptos, if staying 100% Rust matters more than ecosystem maturity.

## ADR-006: SQLite for server storage
**Status:** Accepted
**Context:** Self-hosted deployments should need nothing else to run.
**Decision:** SQLite through `sqlx`, with embedded migrations. Collections stay as files; SQLite holds history, traces, run results, users.
**Consequences:** Simple backups (one directory). Postgres support can be added later behind a trait if needed.

## ADR-007: License
**Status:** Accepted (2026-10-31)
**Decision:** MIT.
**Consequences:** Maximum adoption and reuse. Dependencies must be MIT-compatible; `cargo-deny` enforces an allowlist (MIT, Apache-2.0, BSD, ISC, Zlib, Unicode, MPL-2.0 as a file-level copyleft exception).

## ADR-009: Name and binary
**Status:** Accepted (2026-10-31)
**Decision:** The product is **SnapRequest**. The binary is `snapreq` (not `snap`, which clashes with Ubuntu's Snap package manager). Crates use the `snapreq-` prefix; the collection root file is `snapreq.toml`.

## ADR-008: Declarative assertions first, JS scripting later
**Status:** Accepted
**Context:** Most API tests are simple checks. Embedding a JS engine adds size and security surface.
**Decision:** Phase 5 ships declarative assertions in TOML; Phase 9 adds sandboxed JS via `rquickjs`.
**Consequences:** CI usage is possible early, and scripting can be designed with real user feedback.

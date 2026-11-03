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

## ADR-010: Own connection pool on hyper's low-level connection API
**Status:** Accepted (2026-10-31)
**Context:** hyper-util's pooled `Client` opens connections in the background, separate from the request that
triggered them, so DNS/connect/TLS timings cannot be attributed to a specific request's trace.
**Decision:** Use `hyper::client::conn::http1` (and `http2` later) directly. `snapreq-core` resolves, connects,
handshakes and pools connections itself (keyed by scheme, host, port), and wraps every socket in a metering
I/O adapter that records bytes and the first-read/last-write instants.
**Consequences:** Exact per-request trace events, connection-reuse visibility and byte counts. We own pool
correctness (idle timeouts, closed-connection detection).

## ADR-011: System resolver instead of hickory; ring as the TLS crypto provider
**Status:** Accepted (2026-10-31). Supersedes the DNS row of the PLAN tech table.
**Context:** A debugging tool must resolve names exactly as curl and browsers on the same machine do
(`/etc/hosts`, nsswitch, mDNS, VPN split DNS, corporate resolvers). hickory reads `resolv.conf`-style
configuration and can diverge from the OS. Timing DNS and pinning hosts do not need a custom resolver.
**Decision:** Use the OS resolver (`getaddrinfo` via `tokio::net::lookup_host`), timed as its own phase.
Host pinning (like curl `--resolve`) is implemented in SnapRequest (`RequestSettings::resolve`). Connections use
Happy Eyeballs (RFC 8305 style: interleaved families, 250 ms stagger). TLS uses rustls with the `ring`
provider (pure Rust + asm, no CMake/NASM on Windows), trusting the OS certificate store with Mozilla's
roots (`webpki-roots`) as fallback.
**Consequences:** No resolver drift from the OS. Choosing a specific DNS server or DNS-over-HTTPS would need hickory
later as an opt-in.

## ADR-012: Minimum supported Rust version policy
**Status:** Accepted (2026-10-31)
**Context:** The MSRV started at 1.85 (the minimum for edition 2024). The fix for RUSTSEC-2026-0009 in `time`
(pulled in by `x509-parser`) requires Rust 1.88, and the MSRV-aware resolver kept the vulnerable version locked.
**Decision:** MSRV is now **1.88**. Policy: security fixes always win; otherwise the MSRV may be raised to any
stable release at least 6 months old, and every raise is noted in the changelog. CI checks the MSRV.
**Consequences:** End users install release binaries, so this only affects people building from source.

## ADR-013: toml_edit for writing collection files
**Status:** Accepted (2026-10-31)
**Context:** The `toml` serializer writes `{ value, enabled = false }` entries as `[headers.X]` sub-tables.
TOML requires plain keys before sub-tables, so saving would reorder headers and break the format's
"order is kept" rule. Later, the web UI must edit hand-written files without destroying comments.
**Decision:** Read with `toml` (good error spans); write with `toml_edit`, laying the document out
explicitly (sections, `[[arrays]]`, inline entries inside key/value tables, `"""` multi-line strings).
**Consequences:** Saved files read like hand-written ones and round-trip exactly. In-place edits that
keep comments will use `toml_edit`'s document model (P6/P7).

## ADR-014: Digest auth in-house; md-5 for the legacy hash
**Status:** Accepted (2026-11-03)
**Context:** Digest auth (RFC 7616) is a small algorithm but needs a challenge round trip that must
show up in our trace and respect our redirect and redaction rules; crates such as `diqwest` are tied
to reqwest. Most servers still use MD5, which `ring` does not provide.
**Decision:** Implement challenge parsing and the response computation in `snapreq-core::auth::digest`,
tested against the RFC 7616 and RFC 2617 examples. SHA-256 and SHA-512-256 come from `ring`; MD5 from
RustCrypto's `md-5` (MIT/Apache-2.0, no other new dependencies of note). The retry is recorded as a
`TraceEvent::AuthChallenge`, which splits hops like `Redirect`.
**Consequences:** One small new dependency. `qop=auth-int` is not supported (it needs the whole body
hashed before sending, which conflicts with streamed file bodies); it can be added for in-memory
bodies later. Each challenge uses `nc=00000001`; nonce reuse across requests is a possible
optimization.

## ADR-015: OAuth 2.0 tokens behind a TokenStore; the CLI caches them on disk
**Status:** Accepted (2026-11-03)
**Context:** OAuth 2.0 tokens must survive between requests (and, for the CLI, between runs) so users
are not sent through a token request or browser sign-in every time. `snapreq-core` must stay free of
storage and UI concerns, and the server (P6) will want its own storage.
**Decision:** The core engine runs the grants (client credentials, password, authorization code with
PKCE on a loopback listener, refresh) through the same `Client`, so traces, proxies and TLS settings
apply. Tokens go through a `TokenStore` trait (default: in memory per client); showing the sign-in
URL goes through an `AuthorizationPrompt` trait. The CLI implements both: a JSON file in the user cache
directory (atomic writes, mode 0600, no client secrets or passwords) and printing the URL plus opening
the browser. Approved by the maintainer.
**Consequences:** Access and refresh tokens are stored on disk outside the collection; `snapreq tokens
clear` deletes them. The server can plug in SQLite-backed storage later. Token requests go through
`Client::execute_plain`, which keeps the request futures non-recursive and `Send`.

# SnapRequest — Project Plan

> An open-source, self-hosted HTTP API testing and debugging tool written in Rust.
> Think Postman, but a single fast binary you own: CLI, self-hosted server and web UI,
> with detailed, low-level request logs.

---

## 1. Goals and non-goals

### Goals
1. **Fast and light.** Starts in milliseconds, uses little memory, and runs as one static binary.
2. **Detailed debugging output.** Every request produces a structured trace: DNS lookup, TCP connect,
   TLS handshake (version, cipher, ALPN, certificate chain), request and response bytes on the wire,
   redirects, time to first byte, download time, and sizes. Think `curl -v` that you can read and search.
3. **Self-hosted first.** `snapreq serve` runs a web UI and API on your own machine or server. No cloud
   account, no telemetry.
4. **Git-friendly collections.** Collections are plain-text files on disk, so they diff, review and
   version cleanly.
5. **CI-ready.** `snapreq run` runs collections headless, with assertions, exit codes and JUnit/JSON reports.
6. **Open source**, with a clean crate layout that is easy to contribute to.

### Non-goals (for 1.0)
- A cloud sync service or SaaS offering.
- Full compatibility with Postman's JavaScript sandbox (`pm.*`). We import Postman collections, but
  scripting will be our own API.
- A native desktop app. It may come later through Tauri, reusing the web UI (see Phase 10).

---

## 2. Architecture

```
                 ┌──────────────────────────────────────────────┐
                 │                 snapreq (bin)                 │
                 │   CLI (clap)            serve (axum + web UI) │
                 └───────┬──────────────────────────┬───────────┘
                         │                          │
        ┌────────────────┼──────────────┬───────────┼─────────────────┐
        ▼                ▼              ▼           ▼                 ▼
 snapreq-core     snapreq-collection  snapreq-test  snapreq-server   web/ (SPA,
 (HTTP engine,    (file format,       (assertions,  (REST + WS API,   embedded
  timing trace,    env + variables,    runner,       storage, auth)   into binary)
  auth, TLS)       import/export)      reports)          │
                                                          ▼
                                                   SQLite (history,
                                                   logs, users)
```

### Cargo workspace layout

```
snaprequest/
├── Cargo.toml                 # workspace
├── crates/
│   ├── snapreq-core/          # Request/Response model, HTTP engine, trace events, auth
│   ├── snapreq-collection/    # Collection/environment file format, variable interpolation
│   ├── snapreq-test/          # Assertions, collection runner, reporters
│   ├── snapreq-server/        # axum API, WebSocket live logs, SQLite storage, auth
│   └── snapreq-cli/           # The `snapreq` binary (CLI + `serve` subcommand)
├── web/                       # Frontend SPA, built to static files and embedded with rust-embed
├── docs/                      # PLAN.md, CHECKPOINT.md, DECISIONS.md, user docs (mdBook later)
└── tests/fixtures/            # Sample collections, certificates and so on
```

### Main technical choices (reasoning in DECISIONS.md)

| Concern | Choice |
|---|---|
| Async runtime | `tokio` |
| HTTP engine | `hyper` 1.x + `hyper-util`, **not** `reqwest`. We need hooks into every connection phase to record timings and wire bytes. |
| TLS | `rustls` + `tokio-rustls` (pure Rust, records handshake details), with `webpki-roots` and native certs |
| DNS | OS resolver (timed separately) plus our own host pinning like curl's `--resolve` (ADR-011) |
| Serialization | `serde`, `serde_json`, `toml` |
| Collection format | TOML, one file per request, with folders as directories |
| JSON querying | `serde_json_path` (RFC 9535 JSONPath) |
| Schema validation | `jsonschema` |
| CLI | `clap` (derive), `anstream`/`owo-colors` for color output |
| Logging | `tracing` + `tracing-subscriber` |
| Errors | `thiserror` in libraries, `anyhow` in binaries |
| Server | `axum` + `tower-http` |
| Storage | `sqlx` with SQLite (migrations embedded) |
| Password hashing | `argon2` |
| Scripting (later) | `rquickjs`: sandboxed JS with a time and memory limit |
| Frontend | Svelte 5 + TypeScript + Vite, CodeMirror 6 for editors, embedded with `rust-embed` |
| Testing | `cargo test`, `insta` snapshots, `wiremock` and local hyper test servers, `cargo-nextest` |
| Distribution | `cargo-dist`, Docker image, GitHub Releases |

### The trace model

Every execution emits an ordered stream of `TraceEvent`s. The CLI, the server logs, the web UI
timeline and reports all render from this same stream.

```rust
enum TraceEvent {
    DnsStart { host }, DnsEnd { addrs, duration },
    ConnectStart { addr }, ConnectEnd { local_addr, duration },
    TlsStart, TlsEnd { version, cipher, alpn, peer_certs, duration },
    RequestHeadersSent { raw }, RequestBodySent { bytes },
    ResponseFirstByte { ttfb },
    ResponseHeaders { status, version, headers },
    ResponseBodyChunk { len }, ResponseComplete { total_bytes, duration },
    Redirect { from, to, status },
    ConnectionReused { id },
    Error { phase, message },
}
```

---

## 3. Phases and checkpoints

Each phase is split into checkpoints (`P<phase>.<n>`). A checkpoint is done only when its
**acceptance criteria** pass and `cargo fmt --check`, `cargo clippy -D warnings` and `cargo test` are clean.
Progress is tracked in `docs/CHECKPOINT.md`.

---

### Phase 0: Foundation
Set up the repository so that every later phase builds on solid ground.

- **P0.1** `git init`, Cargo workspace with the 5 crates (empty), `rust-toolchain.toml`, `.gitignore`, `rustfmt.toml`.
- **P0.2** License files, README skeleton, CONTRIBUTING.md, CODE_OF_CONDUCT.md.
- **P0.3** GitHub Actions CI: fmt, clippy, test on Linux, macOS and Windows; `cargo-deny` for licenses and advisories.
- **P0.4** Shared conventions: error types, `tracing` setup, workspace dependency versions.

**Acceptance:** `cargo build --workspace` passes; CI is green; `snapreq --version` prints.

---

### Phase 1: Core HTTP engine (`snapreq-core`)
The heart of the product. Most of the performance and "detailed logs" value is built here.

- **P1.1** Data model: `Request` (method, URL, headers, query, body, settings) and `Response` (status, version, headers, body, timings, sizes). Serde round-trip.
- **P1.2** Basic execution with hyper: all methods, headers, query params, HTTP/1.1, connection pooling.
- **P1.3** Body types: raw text, JSON, `x-www-form-urlencoded`, multipart (with file upload), binary file, streaming large bodies.
- **P1.4** Instrumented connector: timed DNS (OS resolver) with host pinning, Happy Eyeballs TCP connect, and the rustls TLS handshake, each emitted as `TraceEvent`s.
- **P1.5** TLS options: verify on/off, custom CA, client certificates (mTLS), min/max TLS version; capture the peer certificate chain.
- **P1.6** HTTP/2 (ALPN), response decompression (gzip, br, zstd, deflate), redirect policy (follow, max count, keep auth on redirects), timeouts (connect, total), proxy support (HTTP/HTTPS/SOCKS5, `NO_PROXY`).
- **P1.7** Cookie jar (per session, can be persisted).
- **P1.8** Wire capture: exact request and response bytes, redacting secrets when configured.
- **P1.9** Benchmarks (`criterion`) against a local server to keep overhead low.

**Acceptance:** an integration test suite against local HTTP/1.1, HTTP/2 and TLS test servers covers every feature above; each request yields a full, correctly ordered trace.

---

### Phase 2: CLI MVP (`snapreq-cli`)
The first thing people can actually use.

- **P2.1** `snapreq send <METHOD> <URL> [-H k:v] [-q k=v] [-d body | --json | --form | --file]`.
- **P2.2** Pretty output: status line, headers, body with JSON/XML/HTML formatting and syntax colors; automatic pager.
- **P2.3** Verbosity levels: `-v` shows the request and timings, `-vv` shows the full trace, TLS details and raw wire bytes.
- **P2.4** Timing waterfall printed in the terminal (DNS | connect | TLS | TTFB | download).
- **P2.5** Output modes: `--output json` (machine-readable response and trace), `--body-only`, `-o file`.
- **P2.6** Snapshot tests of CLI output (`insta`).

**Acceptance:** `snapreq send GET https://httpbin.org/get -vv` shows a complete, readable trace; `--output json` is stable and documented.

---

### Phase 3: Collections, environments and variables (`snapreq-collection`)

- **P3.1** File format v1: `snapreq.toml` at the collection root, one `.toml` file per request, folders as directories, explicit ordering. Versioned schema.
- **P3.2** Environments (`environments/*.toml`) and variable scopes: global, collection, environment, folder, request, runtime. Clear precedence rules.
- **P3.3** `{{variable}}` interpolation in URL, headers, body and auth. Dynamic variables: `{{$uuid}}`, `{{$timestamp}}`, `{{$randomInt}}` and so on. Clear errors for undefined variables.
- **P3.4** Secrets: variables marked secret are read from a `.env`-style file or OS keychain, never written to collection files, and redacted in logs.
- **P3.5** Inheritance: folder and collection level headers, auth and base URL.
- **P3.6** CLI: `snapreq new`, `snapreq send path/to/request.toml -e dev`, `snapreq ls`.

**Acceptance:** a sample collection in `tests/fixtures` runs request by request against several environments; secret values never appear in output or files.

---

### Phase 4: Authentication helpers (`snapreq-core::auth`)

- **P4.1** Basic, Bearer and API key (header or query).
- **P4.2** Digest auth.
- **P4.3** OAuth 2.0: client credentials, password, authorization code + PKCE (local callback listener), token refresh and caching.
- **P4.4** AWS Signature V4.
- **P4.5** Auth inheritance and the `none`/`inherit` modes in collections.

**Acceptance:** each auth type is tested against a local mock; OAuth tokens are cached and refreshed automatically.

---

### Phase 5: Assertions and the test runner (`snapreq-test`)

- **P5.1** Declarative assertions in request files: status, header, response time, body contains, JSONPath equals/exists/matches/type, JSON Schema.
- **P5.2** Chaining: extract values from a response (JSONPath, header, regex) into runtime variables for later requests.
- **P5.3** `snapreq run <collection> [-e env] [--folder x] [--iterations n] [--data data.csv|json]`. Data-driven runs.
- **P5.4** Run control: bail on first failure, delay between requests, parallel runs of independent requests.
- **P5.5** Reporters: pretty console output, JSON, JUnit XML, single-file HTML report including traces.
- **P5.6** Exit codes for CI; an example GitHub Actions workflow.

**Acceptance:** a CI job runs a fixture collection and publishes a JUnit report; failing assertions show expected vs actual values and the relevant trace.

---

### Phase 6: Self-hosted server (`snapreq-server`)

- **P6.1** `snapreq serve [--bind 127.0.0.1:7878] [--data-dir]`; axum app, config file plus environment variables.
- **P6.2** SQLite storage with migrations: request history, full traces, run results. Retention and size limits.
- **P6.3** REST API: execute a request, CRUD for collections and environments (backed by files in a workspace directory), history search.
- **P6.4** WebSocket endpoint that streams `TraceEvent`s and runner progress live.
- **P6.5** OpenAPI description of our own API (`utoipa`).
- **P6.6** **Outbound request policy (SSRF protection):** a server that sends requests for its users can reach the internal network. Add configurable allow/deny lists for CIDRs and hosts, blocking cloud metadata addresses by default when running in multi-user mode.
- **P6.7** Docker image (distroless, non-root) and a `docker-compose.yml` example.

**Acceptance:** API integration tests pass; `docker run` starts the server and requests execute through the API with live trace streaming.

---

### Phase 7: Web UI (`web/`)

- **P7.1** Project setup: Svelte + TS + Vite, typed API client generated from the OpenAPI spec, built assets embedded into the binary.
- **P7.2** Request builder: method, URL with variable highlighting, params, headers, body editors (CodeMirror), auth tab.
- **P7.3** Response viewer: pretty/raw/preview views, headers, cookies, search inside the body, large-body virtualization.
- **P7.4** **Debug view:** timing waterfall, TLS and certificate details, raw wire log, redirect chain. This is the headline feature.
- **P7.5** Collections sidebar (tree, drag and drop), environment switcher and editor.
- **P7.6** History with search and filters; re-run from history; compare two responses (diff).
- **P7.7** Collection runner UI with live progress and reports.
- **P7.8** Dark and light themes, keyboard shortcuts, accessibility pass.

**Acceptance:** everything the CLI can do for single requests and runs can also be done in the browser.

---

### Phase 8: Multi-user and hardening

- **P8.1** Local accounts (argon2), sessions and API tokens. A single-user mode with no login, for localhost only.
- **P8.2** Workspaces with roles (owner, editor, viewer).
- **P8.3** OIDC login (Keycloak, Authentik, Google and so on).
- **P8.4** Encrypted secret storage on the server.
- **P8.5** Audit log, rate limiting, CSRF protection, security headers.
- **P8.6** Backup and restore (export of the full data directory).

**Acceptance:** a security review checklist is completed; a two-user scenario cannot read the other's workspace.

---

### Phase 9: Interoperability and scripting

- **P9.1** Import: cURL command, Postman Collection v2.1 (with environments), OpenAPI 3.x, Insomnia, HAR.
- **P9.2** Export: cURL, HAR, code snippets (Python requests, JS fetch, Go, Rust reqwest).
- **P9.3** Pre-request and test scripts in JS via `rquickjs`: a `snapreq.*` API (variables, request, response, expect), with time and memory limits and no file or network access.
- **P9.4** Translation of simple Postman `pm.test`/`pm.expect` scripts during import where possible.

**Acceptance:** a real public Postman collection imports and runs; scripts cannot escape the sandbox (tests prove it).

---

### Phase 10: Advanced protocols and features

- **P10.1** WebSocket client (send/receive, message log).
- **P10.2** Server-Sent Events viewer.
- **P10.3** GraphQL: query editor, variables, schema introspection.
- **P10.4** gRPC (`tonic`): server reflection and proto files, unary and streaming.
- **P10.5** Load test mode: `snapreq bench` with concurrency, rate, duration, and latency percentiles (`hdrhistogram`).
- **P10.6** Mock server: serve responses defined in a collection.
- **P10.7** (Optional) Tauri desktop wrapper around the web UI.

---

### Phase 11: Release 1.0

- **P11.1** User documentation site (mdBook): install, CLI reference, file format spec, self-hosting guide.
- **P11.2** Packaging: `cargo-dist` installers, Homebrew, AUR, Scoop, Docker Hub/GHCR, `cargo install`.
- **P11.3** Fuzzing (`cargo-fuzz`) of the collection parser, importers and HTTP response handling.
- **P11.4** Published benchmarks (startup time, memory, request overhead vs curl).
- **P11.5** Freeze file format v1 and the CLI JSON output; semantic versioning policy.
- **P11.6** Release notes, changelog (`git-cliff`), 1.0 tag.

---

## 4. Milestone releases

| Version | After phase | What users get |
|---|---|---|
| 0.1 | 2 | A fast `curl` alternative with detailed debug output |
| 0.2 | 3–5 | Collections, auth and a CI test runner |
| 0.3 | 6–7 | Self-hosted server with web UI |
| 0.4 | 8–9 | Multi-user, Postman/OpenAPI import, scripting |
| 0.5 | 10 | WebSocket, GraphQL, gRPC, load testing |
| 1.0 | 11 | Stable, documented and packaged |

## 5. Principles for every session
- Work one checkpoint at a time. Do not start the next one until the current one passes its acceptance criteria.
- Tests come with features, not after.
- Keep `snapreq-core` free of UI and storage concerns, so the CLI, server and future desktop app all share it.
- Any new architectural decision gets an entry in `docs/DECISIONS.md`.
- At the end of a session, update `docs/CHECKPOINT.md`.

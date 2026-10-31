# Checkpoint — where we are

> Read this first when resuming a session. Update it at the end of every session.

## Current position
- **Phase:** 3 Collections, environments and variables, starting (Phases 0–2 complete)
- **Current checkpoint:** P3.5, inheritance

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
1. P3.5: inheritance in `build_request`: headers and settings from collection → folders (outermost first) → request.
   Headers: inherited entries come first; a request/folder entry with the same name (case-insensitive) replaces
   the inherited one(s); a disabled entry with the same name switches the inherited header off (document).
   Settings: request keys override folder keys override collection keys override core defaults; relative paths
   (ca_cert, client_cert/key) resolve against the file that set them. Auth inheritance comes with Phase 4.
   Update FORMAT.md. Then P3.6 CLI: `snapreq send path/to/request.toml -e dev` / `snapreq run`? (run is P5),
   `snapreq ls`, `snapreq new`, `snapreq secret set NAME [-e ENV]`, `--var name=value`.

## Secret notes (P3.4)
- `crates/snapreq-collection/src/secrets.rs`: `Secrets::load(collection, env, env_vars)` → `get(name)` (`SNAPREQ_SECRET_<NAME>` > `.secrets/<env>.toml` > `.secrets/shared.toml`), `fill(&mut Scopes)` fills secret variables without value. `--var` wins via the runtime scope.
- `store()` writes `.secrets/<env>.toml` (or `shared.toml`), creates `.secrets/.gitignore` (`*`), chmod 600 on Unix.
- Process env is passed in (no `std::env` reads inside the library) so tests stay hermetic.

## Interpolation notes (P3.3)
- `crates/snapreq-collection/src/interp.rs`: `Interpolator` (`{{name}}`, recursive with cycle detection, `\{{` literal, `$uuid`/`$timestamp`/`$isoTimestamp`/`$randomInt`), collects secret values used.
- `crates/snapreq-collection/src/build.rs`: `build_request(collection, key_path, scopes)` → core `Request`; relative paths resolve against the request file's directory; disabled entries not interpolated; secret values → `settings.secrets`. Settings from the request file only (inheritance is P3.5).
- CI on P3.1 failed on Windows: CRLF checkouts broke exact-text fixture assertions. Fixed with `.gitattributes` `* text=auto eol=lf` (verified by simulating CRLF fixtures locally).

## Variable notes (P3.2)
- `crates/snapreq-collection/src/vars.rs`: `Variables` (ordered; scalar or `{ value, secret }`; secret values never serialized), `Environment::{load, list}`, `Scopes::for_request` (runtime > request > folders innermost-first > environment > collection > global; Bruno-like), `lookup` → (Variable, Scope), `visible()`, `set_runtime`.
- `[vars]` in `snapreq.toml`, `folder.toml`, request files; `save_environment` keeps entries inline.
- Secret variables without value (`{ secret = true }`) get their values in P3.4.

## Collection format notes (P3.1)
- Spec: `docs/FORMAT.md`. Crate `snapreq-collection`: `files.rs` (serde types, `deny_unknown_fields`, method validated during parse, `validate()` for content-xor-file etc.), `kv.rs` (KeyValues with hand-written serde: string | list | `{value, enabled}`; numbers/bools accepted), `load.rs` (`Collection::load`, `requests()`, `find("a/b")`, `folders_of`), `save.rs` (toml_edit layout, ADR-013), `error.rs` (file:line:col).
- Fixture collection: `crates/snapreq-collection/tests/fixtures/basic/`.
- Paths in files are kept as written (relative); resolving them against the file's directory happens when building core requests (P3.5).
- First invalid file aborts loading (no partial collections yet); unknown `order` keys are warnings.

## Snapshot notes (P2.6)
- `crates/snapreq-cli/src/snapshot_tests.rs` + `src/fixtures.rs` (deterministic redirected HTTPS exchange and a connect failure). Snapshots in `src/snapshots/`; `.gitattributes` keeps `*.snap` LF.
- Update with `cargo insta test -p snapreq-cli --review` (documented in CONTRIBUTING). CI runs with `CI=true`, so missing/changed snapshots fail.
- Found during review: the `m` summary now says `redirects N (phase timings are for the final hop)`.

## JSON / -o notes (P2.5)
- Naming: `--format pretty|json` and `-o/--output FILE` (curl meaning). PLAN updated.
- `crates/snapreq-cli/src/json_out.rs`: schema v1 (`format`, `request {method,url}`, `response`, `error`, optional `body_file`); documented in `docs/CLI.md`. Secrets scrubbed from request/response URLs; body untouched.
- `-o`: parent dir checked before sending (exit 3); body written after the response (core buffers bodies; streaming to disk is a later improvement); `saved … to …` on stderr when it is a terminal.

## Waterfall notes (P2.4)
- `crates/snapreq-cli/src/waterfall.rs`: `hops()` splits the trace at `Redirect` events; segment start = event time − duration; `render(out, trace, width)` (min width 40); `terminal_width(Stream)` via `terminal_size_of` → `COLUMNS` → 80.
- Shown at the end of `-v` (stderr) and with `--print t` (stdout).

## Verbose notes (P2.3)
- `crates/snapreq-cli/src/trace_view.rs`: `render(out, trace, level)`, curl-style prefixes `*` `>` `<` `!`; `-vv` adds `[ms]` stamps, full cert chain, wire dump (binary bodies summarized).
- Trace goes to stderr; stdout only carries `--print` parts. Failures print the partial trace with `-v`, else a hint.

## Output notes (P2.2)
- `crates/snapreq-cli/src/output.rs`: `Parts` (`--print` letters H B h b m), `print()`, JSON pretty printer (serde_json `preserve_order` enabled in the CLI crate), binary summary on TTY, `human_bytes`.
- Colors via `anstream::AutoStream` (`--color auto|always|never`, `NO_COLOR` honoured). Request headers printed from the trace (already redacted).
- Default parts: `hb` on a TTY, `b` when piped.

## CLI notes (P2.1)
- `crates/snapreq-cli/src/`: `cli.rs` (clap definitions), `send.rs` (`build_request`, pure, unit-tested), `main.rs` (exit codes in `mod exit`, printing).
- Exit codes documented in `docs/CLI.md` (0 ok, 1 internal, 2 usage, 3 invalid request, 4 `--fail` HTTP ≥ 400, 5 network, 6 timeout). Keep `mod exit` and the doc in sync.
- `-d` does not imply a form content type (deliberate difference from curl). URL without scheme → `http://`.
- `-vv` turns on `capture_wire`; `-v` output itself arrives in P2.3.
- End-to-end tests run the real binary (`CARGO_BIN_EXE_snapreq`) against a local hyper server.

## Benchmark notes (P1.9)
- `crates/snapreq-core/benches/client.rs` (criterion, `async_tokio`); results and findings in `docs/BENCHMARKS.md`.
- Found and fixed: Happy Eyeballs used `sleep(0)` for the first attempt (1 ms Tokio timer tick) → fresh connections 1.14 ms → 72 µs.
- Benchmark/test servers must set `TCP_NODELAY`, otherwise HTTP/2 shows ~40 ms delayed-ACK stalls.

## Redaction / wire notes (P1.8)
- `client/redact.rs`: `Redactor` applied by the `Recorder` whenever the trace leaves it (success and failure) — never to sent bytes or to `Response` headers/body.
- `ClientConfig::sensitive_headers` (default `DEFAULT_SENSITIVE_HEADERS`): values redacted with structure kept (`Bearer <redacted>`, `sid=<redacted>`, Set-Cookie attributes kept).
- `RequestSettings::secrets` (`#[serde(skip)]`) hidden anywhere in trace text and wire bytes. Credential values of sensitive request headers (≥ 6 chars), jar cookies and proxy passwords are learned automatically per hop, so echoes in bodies are hidden too.
- `RequestSettings::capture_wire` → `TraceEvent::Wire { sent, received, truncated }` per HTTP/1.1 exchange (inside TLS), capped by `ClientConfig::wire_capture_limit` (1 MiB/direction). Emitted on failure too. HTTP/2: `info` "not available".

## Cookie notes (P1.7)
- `client/cookies.rs`: per-`Client` `Jar` over `cookie_store` (RFC 6265; `public_suffix` feature off, so a server could set a cookie for a public suffix; acceptable for an API tester, documented). `Client::cookies()` / `clear_cookies()`; `StoredCookie` is serde for later persistence (P3/P6).
- `RequestSettings::cookies` (default true). Cookies are stored after every hop (redirect-aware) and sent per hop; trace has `info` events for stored/updated/deleted/rejected and sent cookies.
- User `Cookie` header is merged with jar cookies; user values win per name.

## Proxy notes (P1.6d)
- `RequestSettings::proxy: ProxyMode` = `"env"` (default) | `"direct"` | proxy URL (`http://`, `socks5://`, `socks5h://`, credentials in URL).
- `client/proxy.rs`: `select()` takes an env-lookup closure (tests never set env vars). Env keys: `http(s)_proxy`, `HTTP(S)_PROXY`, `all_proxy`, `ALL_PROXY`; `no_proxy`/`NO_PROXY` (`*`, domain suffix, exact IP, optional port). Loopback targets bypass *environment* proxies (deliberate difference from curl); explicit proxies always apply. Scheme-less env values get `http://`.
- HTTP proxy: plain-HTTP targets sent in absolute form with `Proxy-Authorization` (trace shows `Basic <redacted>`); HTTPS targets use `CONNECT` then TLS inside the tunnel. SOCKS5 implemented in-house (RFC 1928/1929); `socks5` resolves locally, `socks5h` sends the name.
- Pool key includes the proxy. No HTTPS-to-proxy (`https://` proxy URLs) yet; no h2 prior knowledge through HTTP proxies.
- Timings: `connect` is the TCP connect to the proxy; tunnel setup time is in an `info` event.

## HTTP/2 notes (P1.6c)
- ALPN per `http_version`: Auto → `h2, http/1.1`; Http1 → `http/1.1`; Http2 → `h2` (TLS fails in `Phase::Tls` if not selected); plain-TCP Http2 = prior knowledge. Auto over plain HTTP stays HTTP/1.1 (no h2c upgrade).
- `conn::Sender { H1, H2 }`. Pool: HTTP/1.1 idle list (exclusive checkout) + `shared` map of HTTP/2 handles (cloned per request, registered right after connect so concurrent requests share). `PoolKey` includes `http_version`.
- HTTP/2 timing: `SendTiming::Stream` — send = until the request body is fully handed to the stream (`body::timed`), wait = until response headers. Header sizes reported as 0 (HPACK on shared socket).
- HTTP/2 requests use the absolute URI (`:authority`), drop Host and connection-specific headers (rebuilt, not `remove`d, to keep order). Trace shows lowercase HTTP/2 headers.

## Redirect notes (P1.6b)
- `client/redirect.rs::next_hop` (pure, unit-tested) decides the next hop; `Client::follow` loops over hops with one Recorder, so the trace covers the whole chain and `Timings::from_trace` reports the final hop.
- 303 → GET (except HEAD); 301/302 turn POST into GET; 307/308 keep method + body. Body-less hops drop Content-Type/Length.
- Cross-origin hops drop Authorization, Cookie, Proxy-Authorization and an explicit Host header; HTTPS→HTTP is flagged. Notes go to `info` events.
- Exceeding `max_redirects` → `Phase::Redirect` error with trace. `follow_redirects = false` returns the 3xx.
- Auth helpers (Phase 4) must re-apply per hop and respect the cross-origin rule.

## Decompression notes (P1.6a)
- `client/decode.rs`: gzip (multi-member), deflate (zlib, falls back to raw), br, zstd; stacked encodings undone in reverse; pure-Rust decoders (`flate2`, `brotli-decompressor`, `ruzstd`).
- `Accept-Encoding: gzip, deflate, br, zstd` sent when `settings.decompress` and the user did not set one.
- Failure or unknown encoding keeps the raw body plus an `info` event; decoded bodies capped at 1 GiB. `Content-Encoding` header stays in the response headers.

## TLS options notes (P1.5)
- `RequestSettings::tls: TlsSettings { verify, ca_cert, client_cert {cert,key}, min_version, max_version }` (replaced top-level `verify_tls`). Versions serialize as "1.2"/"1.3".
- `TlsContext` (client-wide) caches the OS root store and a default rustls config; non-default settings build a config per request (files re-read each time, so renewed certs are picked up).
- Verification off uses `RecordingVerifier`: accepts any certificate but runs real verification and reports its verdict as an `info` event. Handshake signatures are still checked.
- Pool key includes `TlsSettings` for https, so insecure and verifying connections never mix.
- PEM parsing via `rustls::pki_types::pem::PemObject` (`rustls-pemfile` is unmaintained, RUSTSEC-2025-0134).
- MSRV raised to 1.88 (ADR-012) for the `time` advisory fix. `scripts/check.sh` runs all CI checks locally (cargo-deny and the 1.88 toolchain are installed on this machine).

## TLS / connect notes (P1.4)
- `client/tls.rs`: rustls with ring provider; OS trust store (`rustls-native-certs`), Mozilla roots fallback; `ClientConfig::extra_root_certificates` (`Certificate::from_pem/from_der`). ALPN offers only `http/1.1` until HTTP/2 (P1.6).
- `Client::new` now returns `Result` (invalid extra roots); `Client::default()` is infallible.
- Trace: `tls_start`/`tls_end` with version, cipher, ALPN and full certificate chain (subject, issuer, validity RFC 3339, serial, SANs, SHA-256 fingerprint). The meter wraps the TLS stream, so byte counts are plaintext HTTP bytes.
- `RequestSettings::resolve` pins hosts (curl `--resolve` syntax via `FromStr`); trace shows an `info` event and no DNS events. TLS verifies against the URL host.
- Happy Eyeballs in `conn.rs`: families interleaved, 250 ms stagger, immediate next attempt on failure, losers reported as `info`.
- `HttpVersionPref::Http2` returns `Error::Unsupported` until P1.6.
- Redirects are not followed yet (P1.6), even though `follow_redirects` defaults to true.

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
| 1 Core HTTP engine | ✅ done |
| 2 CLI MVP | ✅ done |
| 3 Collections & environments | 🟨 in progress (P3.1–P3.4 done) |
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
- P1.4 — 2026-10-31 — HTTPS via rustls with traced handshake and certificate chain, host pinning, Happy Eyeballs. ADR-011 (OS resolver, ring). 69 tests total.
- P1.5 — 2026-10-31 — Per-request TLS options: verify off (with verdict in trace), CA file, mTLS, min/max version; pool separation. Fixed cargo-deny (rustls-pemfile unmaintained, time advisory → MSRV 1.88). 79 tests total.
- P1.6a — 2026-10-31 — Response decompression (gzip, deflate, br, zstd). 86 tests total.
- P1.6b — 2026-10-31 — Redirect following with browser-compatible method rules and cross-origin credential stripping. 100 tests total.
- P1.6c — 2026-10-31 — HTTP/2 via ALPN and prior knowledge, shared multiplexed connections. Verified against example.com, google.com, github.com. 108 tests total.
- P1.6d — 2026-10-31 — Proxies: HTTP forward + CONNECT, SOCKS5/SOCKS5h, env vars with NO_PROXY. P1.6 complete. 122 tests total.
- P1.7 — 2026-10-31 — Cookie jar (cookie_store), redirect-aware, merged with user Cookie headers. 128 tests total.
- P1.8 — 2026-10-31 — Wire capture (HTTP/1.1) and trace redaction (sensitive headers, secrets, auto-learned credentials). 140 tests total.
- P1.9 — 2026-10-31 — Criterion benchmarks; fixed a 1 ms delay on every new connection. Phase 1 complete. 141 tests total.
- P2.1 — 2026-10-31 — `snapreq send` with request, connection, TLS and output options; exit codes; docs/CLI.md. 152 tests total.
- P2.2 — 2026-10-31 — Pretty output: --print parts, colors, JSON formatting, binary summaries. 160 tests total.
- P2.3 — 2026-10-31 — Verbose trace on stderr (-v compact, -vv timestamps, cert chain, wire bytes). 165 tests total.
- P2.4 — 2026-10-31 — Timing waterfall (per hop, terminal-width aware) in -v and --print t. 168 tests total.
- P2.5 — 2026-10-31 — `--format json` (schema v1) and `-o FILE`. 173 tests total.
- P2.6 — 2026-10-31 — insta snapshot tests for all CLI output. Phase 2 complete. 181 tests total.
- P3.1 — 2026-10-31 — Collection format v1 spec (docs/FORMAT.md), loading with file:line errors, stable saving (toml_edit, ADR-013). 196 tests total.
- P3.2 — 2026-10-31 — Environments, `[vars]` tables and scope precedence. 203 tests total.
- P3.3 — 2026-10-31 — `{{var}}` interpolation, dynamic variables, building core requests from collections. 212 tests total.
- P3.4 — 2026-10-31 — Secrets from SNAPREQ_SECRET_* env vars and git-ignored `.secrets/` files. 217 tests total.

## Session log
- **2026-10-31 (1):** Planning. Wrote PLAN.md, DECISIONS.md, CHECKPOINT.md, CLAUDE.md.
- **2026-10-31 (2):** Decisions settled (SnapRequest / MIT / Svelte+TS / TOML). Renamed docs. Did P0.1, P0.2, P0.4 and wrote P0.3. `cargo fmt`, `clippy -D warnings` and `cargo test` all pass; `snapreq --version` prints `snapreq 0.1.0`.
- **2026-10-31 (3):** `LICENSE` and `authors` set to Aryan Fredericks; repo URLs in `Cargo.toml` point to github.com/aryanfredericks/snaprequest; added `origin` remote; renamed the local folder to `snap-request`.
- **2026-10-31 (4):** First commit pushed to GitHub (authored by Aryan Fredericks only; no Claude attribution). CI all green, Phase 0 done. P1.1 done: data model in `snapreq-core` (added `http`, `url`, `bytes`, `base64` deps; `clippy.toml` allows unwrap in tests).
- **2026-10-31 (5):** P1.1 committed and pushed (CI green). P1.2 done: HTTP/1.1 engine (ADR-010: own pool on hyper's low-level conn API). Verified against http://example.com.
- **2026-10-31 (6):** P1.2 committed and pushed (CI green). P1.3 done: streaming bodies (added `futures-util`, `tokio-util`, `mime_guess`).
- **2026-10-31 (7):** Maintainer asked for autonomous flow (commit+push after each passing checkpoint, no asking). P1.3 pushed (CI green). P1.4 done; real HTTPS to example.com verified.
- **2026-10-31 (8):** P1.4 CI failed only on cargo-deny (unmaintained rustls-pemfile, vulnerable time). Fixed in the P1.5 commit; added `scripts/check.sh`.
- **2026-10-31 (9):** P1.6a–P1.9 done and pushed (decompression, redirects, HTTP/2, proxies, cookies, wire capture/redaction, benchmarks). Phase 1 complete.
- **2026-10-31 (10):** Phase 2 (CLI MVP) complete: send command, pretty output, verbose trace, waterfall, JSON output, snapshots. CI watcher now polls once a minute (GitHub's 60 requests/hour unauthenticated limit).
- **2026-10-31 (11):** P3.1 CI failed on Windows (CRLF fixtures) — fixed in the P3.3 commit. GitHub API rate limit was exhausted by CI polling; watcher now polls every 60 s.

# Checkpoint — where we are

> Read this first when resuming a session. Update it at the end of every session.

## Current position
- **Phase:** 6 Self-hosted server, in progress (Phases 0–5 complete)
- **Current checkpoint:** P6.5, OpenAPI description

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
- MSRV is 1.89 (ADR-019); `scripts/check.sh` checks it with the local 1.89 toolchain.

## Next steps
1. P6.5 OpenAPI: `GET /api/v1/openapi.json` describing every endpoint (utoipa derive on handlers + DTOs; core
   types as free-form objects to keep utoipa out of snapreq-core). Test: the document parses and lists every
   route of `api::routes()`.
2. P6.6 SSRF policy (connect-time IP check in core; defaults per ADR-018 mode), P6.7 Docker.

## WebSocket notes (P6.4)
- `snapreq_server::ws` (`GET /api/v1/ws`): per message `{type: execute|run, id?, ...body}`; events forwarded from an
  unbounded mpsc channel fed by the `TraceSink` and `ProgressSink`; `select!` stops polling the channel once it is
  closed (all senders can be dropped before the job finishes; polling a closed channel first starved the job and
  hung the first version of the test). Results are stored like the REST endpoints.

## API notes (P6.3)
- `snapreq_server::api` (`docs/SERVER.md`): execute (raw `Request` or collection key + env + vars, via `Session`),
  history list/search/get, runs (`resolve_unit` shared with the CLI; data rows / iterations / bail / delay /
  parallel; stored with `RunEntry`), file-backed CRUD for collections, requests, flows, environments in
  `Config::workspace` (`--workspace`, `SNAPREQ_WORKSPACE`, `workspace`, default `<data dir>/workspace`).
- Safety: ids/keys validated per segment (`valid_segment`, reserved names), files validated (`RequestFile::validate`,
  `FlowFile::validate` now pub) before writing; JSON errors `{error:{kind,message}}`.
- Core: `TraceSink` + `Client::execute_with` (live, redacted events). snapreq-test: `Live { trace, progress }`,
  `Progress::{RequestStarted, RequestDone}`, `Session::{trace, progress, iteration}`, `Plan::live`; `execute` now uses
  an explicit `FuturesOrdered` loop (the stream-combinator version was not `Send`-general for axum handlers).

## Storage notes (P6.2)
- ADR-019: `rusqlite` (bundled) instead of sqlx; MSRV 1.89 (Cargo.toml, CI job, CONTRIBUTING; check.sh reads it).
- `snapreq_server::store`: `Store::{open, in_memory}` (one `Connection` behind `Arc<Mutex>`, calls via
  `spawn_blocking`; WAL, busy_timeout), migrations as SQL strings tracked by `PRAGMA user_version` (newer schema →
  refuse). Tables `history` (summary columns + `request_json`/`response_json`, `size_bytes`) and `runs` (summary +
  `report_json`). API: `add_history`, `history(HistoryQuery { url_contains, method, status, before_id, limit })`,
  `history_entry`, `add_run`, `runs`, `run_report`, `prune(Retention { max_age, max_bytes })`.
- Redaction before writing: `HistoryEntry::from_exchange(req, result, source)` hides `Request::secrets()` +
  `report::header_credentials(req)` (credential parts of sensitive request headers, ≥ 6 chars, also used by reports
  now), sensitive header values → `<redacted>`, auth → its kind, body ≤ 1 MiB (base64 for binary). Runs store
  `Report::json(true)`.
- Server: opens `<data dir>/snapreq.db` in `Server::bind`, prunes at start and hourly; config `[history] max_age_days`
  (default 30) / `max_mb` (default 512), `0` = unlimited.

## Server notes (P6.1)
- `snapreq-server`: `Config { bind, data_dir, allow_remote }` via `Config::load(&Overrides, env, default_data_dir)`
  (flags > `SNAPREQ_BIND`/`SNAPREQ_DATA_DIR`/`SNAPREQ_CONFIG`/`SNAPREQ_ALLOW_REMOTE` > `config.toml` > defaults;
  `deny_unknown_fields`). Non-loopback binds need `allow_remote` (ADR-018).
- `app(Arc<AppState>) -> Router` (`/healthz`, `/api/v1/version`, `TraceLayer` at INFO); `Server::bind(config)`
  creates the data dir and binds; `local_addr()`; `run(shutdown)` with graceful shutdown; `shutdown_signal()` =
  Ctrl-C or SIGTERM.
- CLI `snapreq serve [--bind] [--data-dir] [--config] [--allow-remote]`; `--log-level` is now optional (default
  `warn`, `info` for serve). Deps: axum 0.8 (no default features: http1, json, tokio, query), tower-http 0.7 (trace).
- Fixed after P5.7 CI failed: `scripts/api-tests.sh` built only the example (`--example` narrows a build).

## CI example notes (P5.7)
- `crates/snapreq-test/examples/mock_shop.rs` (login → token, orders CRUD, order ready on the 3rd poll), collection
  `examples/shop/` (health, auth/login with secret password + secret token capture, orders/list) and flow
  `orders/checkout.flow.toml` (login → create → poll with retry-until → cancel → 404).
- `scripts/api-tests.sh` builds, starts the mock, runs the collection and the flow with JUnit + HTML reports into
  `target/api-reports/`; also part of `scripts/check.sh`. CI job `api-tests` runs it, publishes JUnit with
  `mikepenz/action-junit-report@v5` (needs `checks: write`) and uploads the reports.
- `docs/CI.md`: exit codes for send and run, secrets via `SNAPREQ_SECRET_*`, a GitHub Actions workflow to copy.
- Found while doing this: a request's own secret capture (the login token) showed in its own checks in reports;
  `report::secrets_of` now adds the run's secret captures, `hide` also hides JSON-escaped forms, detailed JSON
  traces are scrubbed; `snapreq send` got the same fix.

## Report notes (P5.6)
- `snapreq_test::report`: `Report::new(collection, unit, iterations, planned, secret_vars)` → `json(details)`,
  `junit()`, `html()`; helpers `hide`, `hide_url`, `scrub_outcomes`, `captures_json`, `lines`, `counts`,
  `request_line`, `secrets_of` (the CLI's run output and `assert_view` now use these).
- Reports hide known secrets in response bodies too (64 KiB body cap); HTML is self-contained (inline CSS, no
  scripts), failures expanded, waterfall bars from `Timings`, readable trace lines.
- CLI `--report junit:FILE|html:FILE|json:FILE` (repeatable, creates directories, "report written to" on stderr;
  write failure → exit 1).

## Run control notes (P5.5)
- `RunOptions { bail, delay, parallel }` on `Plan`. `execute` drives iterations with
  `futures_util::stream::buffered(parallel)` in one task (results in order); with `bail` a shared flag stops new
  iterations and `Requests` iterations stop at the first failing request. `UnitRun::Requests { runs, total }`.
- `run_flow_with(session, flow, adjust, delay)` adds the pause between steps (`run_flow` = no delay).
- CLI: `--bail`, `--delay MS`, `--parallel N`; skipped requests/iterations are reported; JSON `planned_iterations`,
  `passed` false when iterations were skipped.

## Run notes (P5.4)
- `snapreq_test::run`: `Unit::{Flow(&FlowItem), Requests(keys)}`, `Plan { collection, environment, secrets, client,
  overrides, hidden, rows, iterations }`, `execute(plan, unit, adjust) -> Vec<Iteration { number, data, result:
  UnitRun::{Flow, Requests} }>`; a fresh `Session` per iteration. `request_keys(collection, folder)`.
- `Session::data` layer (data row): below `--var` and step vars, above captures.
- `snapreq_test::data::{load, from_csv, from_json}`: own RFC 4180 reader (BOM, quotes, CRLF, multi-line fields).
- CLI `run` targets: flow file, collection/folder directory, `-c DIR [KEY]`, `--folder`, none (cwd's collection);
  `-n`, `--data` (conflict). JSON is uniform: `{ format, target { kind, key, name }, passed, iterations: [{ number,
  data, passed, total, skipped, steps: [...] }] }`. Exit code = worst problem (3 > 5 > 6 > 7).

## Flow notes (P5.3)
- Format (ADR-017): `snapreq_collection::flow::{FlowFile, StepSpec, RetrySpec, FLOW_SUFFIX}`; loader collects
  `*.flow.toml` into `Collection::flows` (`FlowItem { key, path, file }`, `find_flow`), never into the request tree.
  `StepSpec::inline_request()` → `RequestFile`; validation names the step (`step 2 (Create order): ...`).
- `build_file(collection, key, &RequestFile, path, scopes)` builds non-item requests with the folder inheritance of
  `key` (inline steps use the flow's key); `interpolate_assertions(spec, file, scopes)`; auth via `effective_auth_with`.
- `Session`: `overrides` (`--var`) > step vars > `runtime` (captures) > flow vars; `send_spec(SendSpec { target:
  Request(key) | Inline { key, file, path }, lower_vars, upper_vars, assert, capture, until }, adjust)`;
  `RequestRun::until` holds retry-until outcomes. `run_flow(session, flow, adjust) -> FlowRun { steps: StepRun {
  number, title, attempts, run }, total }`: delay, retry (also after network errors, not for invalid steps), stop at
  the first failing step.
- CLI `snapreq run FLOW` (`run_cmd.rs`): flow file path or key (`-c DIR` / from inside the collection); pretty output
  per step (failures always, everything with -v, failing trace with -vv), JSON summary, exit 0/7/3/5/6.
  `snapreq ls` lists flows. Verified against httpbin.org.

## Capture notes (P5.2)
- Format: `snapreq_collection::capture::{Captures, CaptureSpec, CaptureSource}`; `RequestFile::capture`; plain string =
  JSONPath; exactly one source (json/header/cookie/regex+group/status/body), `secret`; validated at load.
- `snapreq_test::Capturer::{compile, extract}` → `Captured { name, value, secret, error }` (`shown()` redacts).
- `snapreq_test::Session` (collection, env, secrets, client; `runtime`/`globals` Variables): `send(key, adjust)` builds
  with the runtime scope, hides earlier secret values, evaluates assertions, stores captures
  (`Variables::set_secret` for secret ones). `RequestRun { request, outcome: Response | Invalid | Failed, assertions,
  captures }`, `passed()`.
- CLI `send`: captures listed with `-v` and in JSON `captures`; invalid capture specs exit 3 before sending.

## Assertion notes (P5.1)
- Format (ADR-016): `snapreq_collection::assert::{AssertSpec, Check, Checks, JsonType}`; `RequestFile::assert` is a
  `Box<AssertSpec>` (keeps `Item` small). `Check` has a custom serde (plain value = `equals`; tables via a
  `#[serde(remote)]` mirror with `deny_unknown_fields`). `try_map_strings` drives interpolation;
  `build_assertions(collection, key, scopes)` interpolates and resolves `schema` against the request file.
  Saving keeps `[assert.json]` / `[assert.headers]` entries inline (`save.rs` layout list).
- Evaluation: `snapreq_test::Assertions::compile(&spec)` (JSONPath, regex, schema file validated up front) →
  `evaluate(&Response) -> Vec<Outcome { subject, check, passed, actual, message }>`. Status accepts `"2xx"` and
  lists; `loose_eq` compares numbers by value and lets text match unquoted numbers/booleans; multi-match JSONPath
  → array; zero matches → missing.
- CLI: `snapreq send` runs a request file's checks (stderr lines + summary, `assert_view.rs`), exit code 7 on
  failure, `--no-assert`, `assertions` in `--format json`; actual values are scrubbed with `Request::secrets()`.
- Deps: `serde_json_path`, `regex`, `jsonschema` (no default features). `deny.toml` now allows `MIT-0`
  (borrow-or-share via jsonschema).

## Auth inheritance notes (P4.5)
- `snapreq_collection::effective_auth(collection, key)` → innermost non-`inherit` `[auth]` and its file;
  `build_request` uses it. `AuthSpec::summary()` gives a secret-free one-liner.
- `snapreq ls` appends `  [auth: …]` to the collection, folder or request that sets auth itself.

## SigV4 notes (P4.4)
- `Auth::AwsSigV4 { access_key, secret_key, session_token, region, service }` (JSON/TOML `type = "aws_sigv4"`).
- `src/auth/sigv4.rs`: pure signer (`canonical_request`, `authorization`, `strict_query`, `amz_date`); unit tests
  use expected values computed with botocore 1.43 (get-vanilla equals the AWS test suite). Non-S3 canonical URI
  is the wire path encoded again; S3 uses it as is. All sent headers are signed except `UNSIGNED` (user-agent,
  expect, connection-specific, authorization...).
- `build::sign_sigv4` runs in `prepare` (so every hop is re-signed): adds `x-amz-date`, `x-amz-security-token`,
  `x-amz-content-sha256` (S3 only, or kept if set by hand, e.g. `UNSIGNED-PAYLOAD`), hashes the body
  (`EncodedBody::sha256_hex` reads files). `auth::apply_to_url` rewrites the query into strict encoding for
  SigV4 (via `effective_url` and redirect targets) so the wire matches the signature.
- `tests/sigv4.rs`: mock service that independently re-verifies signatures from the received request.
- CLI `--aws-sigv4 REGION/SERVICE`, credentials from `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` /
  `AWS_SESSION_TOKEN` (no profiles/SSO yet). Not done: presigned URLs, SigV4a, chunked S3 uploads.
- Reference vectors: botocore installed in a throwaway venv in the session scratchpad (not a project dependency).

## OAuth 2.0 notes (P4.3)
- Model: `Auth::OAuth2(OAuth2 { grant, token_url, auth_url, client_id, client_secret, scope, username, password,
  redirect_uri, client_auth: basic|body, params })`, JSON `"type": "oauth2"`. `Phase::Auth`.
- `snapreq_core::auth` is public now: `Token`, `TokenStore` (+ `MemoryTokenStore`), `AuthorizationPrompt`;
  `ClientConfig::token_store` / `authorization_prompt`. Cache key = grant + token URL + client id + scope + user.
- `src/auth/oauth2.rs`: token request building (client id/secret form-encoded before Basic, RFC 6749 2.3.1),
  response parsing (`expires_in` as number or string; error/error_description), PKCE (RFC 7636 vector tested),
  loopback listener (`redirect_listener`, hand-written HTTP read; 404 for other paths; checks `state`).
- `src/client/oauth.rs`: `oauth_token` (cache → refresh, keeping the old refresh token if none returned → grant).
  Token calls use `Client::execute_plain` (no OAuth code path) so futures are not recursive and stay `Send`
  (test `execute_futures_stay_send`). `execute` retries once with a new token when a *cached* token gets 401,
  re-running the whole chain from the original URL. OAuth tokens are passed into `follow` and re-sent on
  same-origin redirect hops. Expiry margin 10 s; authorization wait 300 s.
- CLI `src/tokens.rs`: `FileTokenStore` (`oauth-tokens.json`, read on each get, atomic write via temp + rename,
  0600), `cache_dir` (`SNAPREQ_CACHE_DIR` > platform), `BrowserPrompt` (`SNAPREQ_NO_BROWSER`), `snapreq tokens
  clear|path`. Flags `--oauth2 GRANT --token-url --auth-url --client-id --client-secret --scope --redirect-uri`;
  password grant takes `-u` (prompted like Basic).
- Not done: OpenID discovery, device code grant, token introspection; nothing exercised against a real
  provider yet (mock-tested only).

## Digest notes (P4.2)
- `src/auth/digest.rs`: `parse_challenges` (several challenges per header, quoted strings, token68 skipped),
  `authorization(challenges, Credentials)` picks the first answerable Digest challenge. MD5 (`md-5` crate,
  ADR-014), SHA-256 / SHA-512-256 (ring), `-sess`, `qop=auth`, `userhash`, `username*` for non-ASCII names.
- `auth::digest_retry` runs in `Client::follow` after each hop: a 401 with a Digest challenge is answered once
  per hop (`answered` flag), recorded as `TraceEvent::AuthChallenge { status, scheme }`. The hop is re-prepared
  with `prepare_hop(req, url, Some(answer))`, so bodies are re-encoded (files reopened). `build::prepare` now
  takes the auth header as a parameter.
- `Timings::from_trace`, the CLI waterfall and the `m` summary treat `AuthChallenge` as a hop boundary.
- Verified against https://httpbin.org/digest-auth/auth/user/passwd/SHA-256 (HTTP/2).

## Auth notes (P4.1)
- Model: `snapreq_core::model::Auth` (`None`, `Basic`, `Bearer`, `ApiKey { name, value, location }`), field
  `Request::auth` (JSON `{"type": ...}`, omitted when none). `Request::secrets()` = `settings.secrets` + auth
  credentials (password, base64 Basic credential, token, key); used by the engine's redactor and CLI scrubbing.
- Engine: `src/auth.rs` (crate-private). Header added in `build::prepare` after user headers unless an enabled
  user header has the same name (then an `info` event says auth was not applied). Query keys are added in
  `Request::effective_url()` and re-applied to redirect targets (replacing same-name params, so never doubled).
  `redirect::next_hop` sets `auth = None` on a cross-origin hop with a note. Invalid header characters give an
  error that does not echo the credential.
- Redactor now also hides the form-urlencoded and percent-encoded forms of every secret (query values in URLs).
- Collection: `AuthSpec` (`inherit` | `none` | `basic` | `bearer` | `api_key`) as `[auth]` in all three file kinds;
  `build_request` picks the innermost non-`inherit` and interpolates only that one (errors name its file).
- CLI: `-u USER[:PASS]` (prompts with rpassword on a TTY when no `:`), `--bearer`, `--api-key NAME=VALUE`
  `--api-key-in header|query`, `--no-auth`; mutually exclusive via clap `conflicts_with`.

## CLI collection notes (P3.6)
- `crates/snapreq-cli/src/collection_cmd.rs`: `resolve_target` (`-c DIR KEY`, a `.toml` path inside a collection, or a key from inside a collection dir; otherwise URL), `build` (load → env → `--var` runtime scope → `Secrets::fill` → `build_request` → `apply_overrides`), `ls`, `new_collection`, `new_request`, `secret_set` (rpassword prompt or stdin).
- `send.rs`: `apply_overrides` only touches what was given on the command line (explicit flags win over files).
- A `--var` value for a variable declared secret anywhere is added to `settings.secrets` (`Scopes::is_declared_secret`).
- Globals scope is still empty in the CLI (a user-level globals file can come later, e.g. with the server).

## Inheritance notes (P3.5)
- `build_request` layers collection → folders (outer→inner) → request. Headers: `merge_headers` removes inherited names (case-insensitive) that a layer sets again, then appends the layer (disabled entries switch inherited headers off). Settings: `Builder::settings(layer, base)` applied per layer with that layer's file/dir for paths and error messages.

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
| 3 Collections & environments | ✅ done |
| 4 Auth helpers | ✅ done |
| 5 Assertions & runner | ✅ done |
| 6 Self-hosted server | 🟨 |
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
- P3.5 — 2026-10-31 — Header and settings inheritance from collection and folders. 220 tests total.
- P3.6 — 2026-10-31 — CLI: send collection requests (-c/-e/--var), ls, new, secret set. Phase 3 complete. 227 tests total.
- P4.1 — 2026-11-03 — Basic, Bearer and API-key auth in core, collections (`[auth]` with inherit/none) and CLI; URL-encoded secrets redacted. 247 tests total.
- P4.2 — 2026-11-03 — Digest auth (RFC 7616) with challenge retry traced as `auth_challenge`; ADR-014 (md-5). 256 tests total.
- P4.3 — 2026-11-03 — OAuth 2.0 (client credentials, password, authorization code + PKCE, refresh, 401 retry); `TokenStore`/`AuthorizationPrompt` in core, file cache + `snapreq tokens` in the CLI; ADR-015. 275 tests total.
- P4.4 — 2026-11-03 — AWS Signature V4 (every hop signed, strict query encoding, S3 payload hash), verified against botocore vectors and a re-verifying mock. 281 tests total.
- P4.5 — 2026-11-03 — `effective_auth` and auth markers in `snapreq ls`. Phase 4 complete. 282 tests total.
- P5.1 — 2026-11-03 — Declarative `[assert]` checks (status, duration, body, headers, JSONPath, JSON Schema) evaluated by `snapreq-test`; `snapreq send` reports them, exit code 7; ADR-016. 293 tests total.
- P5.2 — 2026-11-03 — `[capture]` (JSONPath, header, cookie, regex, status, body; secret captures) and `snapreq_test::Session` passing captured values to later requests. 297 tests total.
- P5.3 — 2026-11-03 — Flows (`*.flow.toml`): chained steps (collection or inline requests) with captures, checks, delays and retry-until polling; `snapreq run FLOW`; ADR-017. 302 tests total.
- P5.4 — 2026-11-03 — `snapreq run` for collections and folders, `--iterations`, data-driven runs (`--data` CSV/JSON). 306 tests total.
- P5.5 — 2026-11-03 — Run control: `--bail`, `--delay`, `--parallel` (concurrent iterations, ordered results). 308 tests total.
- P5.6 — 2026-11-03 — Reports: JUnit XML, self-contained HTML and detailed JSON (`snapreq run --report`), secrets hidden everywhere including bodies. 309 tests total.
- P5.7 — 2026-11-03 — Example collection + flow against a mock in CI with JUnit publishing, docs/CI.md; fixed secret captures showing in their own checks. Phase 5 complete. 310 tests total.
- P6.1 — 2026-11-03 — `snapreq serve`: axum server with config file / env / flags, graceful shutdown, health and version endpoints, loopback-only by default (ADR-018). 319 tests total.
- P6.2 — 2026-11-03 — SQLite storage (rusqlite, ADR-019, MSRV 1.89): redacted request history and run results, migrations, retention by age and size. 325 tests total.
- P6.3 — 2026-11-03 — REST API: execute, history, runs, collections/requests/flows/environments CRUD over the workspace; docs/SERVER.md. 329 tests total.
- P6.4 — 2026-11-03 — WebSocket endpoint streaming live trace events and run progress, then the stored result. 330 tests total.

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
- **2026-10-31 (12):** Phase 3 (collections, environments, variables, secrets, inheritance, CLI) complete. Windows CRLF CI failure fixed and confirmed green.
- **2026-11-03 (13):** P4.1 and P4.2 done (auth model, engine application with redirect rules, collection `[auth]` inheritance, CLI flags). Found and fixed: secrets inside URLs were only hidden in their raw form, not URL-encoded. Digest verified against httpbin.org.
- **2026-11-03 (14):** Maintainer approved caching OAuth tokens in the user cache dir. P4.3, P4.4 and P4.5 done; Phase 4 complete.
- **2026-11-03 (15):** Maintainer asked for flows (chain testing, Hurl-like) in the CLI and web UI: added to PLAN as P5.3 (later P5 items renumbered), P7.7 (flow editor) and P9.1 (Hurl import). P5.1–P5.7 done; Phase 5 complete.
- **2026-11-03 (16):** P5.5–P5.7 done, Phase 5 complete (P5.7 CI first failed: api-tests.sh did not build the CLI; fixed in 3f98d43). Started Phase 6: P6.1 done. Maintainer chose rusqlite over sqlx; P6.2 done.

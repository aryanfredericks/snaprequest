# Checkpoint — where we are

> Read this first when resuming a session. Update it at the end of every session.

## Current position
- **Phase:** 10 Advanced protocols and features (Phases 0–9 complete)
- **Current checkpoint:** P10.4 gRPC (tonic): reflection and proto files, unary and streaming. P10.1–P10.3 done.

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
- `cargo-deny` is installed locally (`scripts/check.sh` runs it; 9 duplicate-version warnings, no errors).
- MSRV is 1.89 (ADR-019); `scripts/check.sh` checks it with the local 1.89 toolchain.

## Next steps
1. P10.4 gRPC, then P10.5 bench, P10.6 mock server, P10.7 (optional) Tauri.
2. Possible follow-ups from docs/SECURITY-REVIEW.md "Accepted risks": key rotation for sealed secrets,
   an admin role (server-wide audit, recovering ownerless workspaces).

## GraphQL notes (P10.3)
- Core `Body::Graphql { query, variables, operation_name }` + `model::graphql_document`, `Body::text()` (ADR-035).
  Format `BodySpec::Graphql` (`query`/`query_file`, `variables`, `operation_name`); `RequestFile::method()` is POST for
  it. CLI `--graphql/--variables/--operation`, `snapreq introspect` (`graphql_cmd.rs`).
- `snapreq-interop::graphql` (`INTROSPECTION_QUERY`, `schema_of`, `to_sdl`); fixture
  `tests/fixtures/graphql-introspection.json` (also served by mock_shop `POST /graphql`).
- Server `graphql.rs` (`POST /requests/graphql-schema`); `export::build` is now shared. UI: `editor/graphql.ts` (lazy
  cm6-graphql + graphql-js chunk), `CodeEditor` `schema` prop, `GraphqlSchema` browser, `app.fetchSchema`.

## SSE notes (P10.2)
- Core (ADR-034): `BodyObserver` + `Streaming { observer, stop: CancellationToken, keep_limit }` in
  `client/mod.rs`, carried in the `Recorder` (`rec.stream`) to `exchange`; `Client::execute_streaming`;
  `snapreq_core::sse` (`SseParser`, `SseEvent`, `TimedEvent`, `observer`, `is_event_stream`).
- `Session.stream`; server `execute_body(.., stream)`, `events` in execute results (`events_json`), `ws.rs`
  split socket with a queue, `event` messages and `cancel`. CLI `snapreq sse` (`sse_cmd.rs`).
- UI: `JobHandlers.onEvent/control`, `RequestTab.liveEvents/stop`, Stop button, `EventsView` tab. mock_shop
  `GET /events?n=N` (boxed unsync bodies).

## WebSocket notes (P10.1)
- `snapreq-core::client::ws` (ADR-033): `Client::websocket(req, WsOptions, sink)` sends the handshake through the
  engine (http1 connections now run `with_upgrades`), checks the accept key, then `tokio-tungstenite` (framing
  only); `WsMessage` (serde `kind`, base64 binary), `split()` into `WsSender`/`WsReceiver`. `Request::effective_ws_url`.
- Format: `[websocket]` (`subprotocols`, `[[websocket.messages]]` name/kind/content), `RequestFile::kind()`;
  `websocket_messages` resolves them. `request_keys` skips WebSocket items; `Session::send` refuses them;
  `Session::websocket` builds one (pre-request scripts via the new `build_for`).
- CLI `snapreq ws` (`ws_cmd.rs`, flattened SendArgs; `collection_cmd::build_full`). Server `ws_session.rs`
  (`/api/v1/ws/websocket`, same-origin check for every `/api/v1/ws*`). UI: `state/websocket.svelte.ts`
  (`WsSession` on every RequestTab), `WebSocketPane`, `WsMessagesEditor`; mock_shop has `GET /ws`.

## Postman script notes (P9.4)
- `snapreq-interop::postman_script::translate` (ADR-032): ordered regex rewrites of `pm.*`/legacy API to
  `snapreq.*`; leftover `pm.`/`postman.`/`require`/`tests[` lines are commented out when balanced, otherwise
  the whole script is kept as a comment; the result is compiled with `snapreq_script::check_syntax`.
  Collection, folder and request events map to `[script]`; every loss is an import warning.
- `prelude.js`: `expect(fn).to.throw()`/`toThrow()`, Chai `.property(name)` now changes the subject.
- The CLI acceptance test uses a postman-echo stand-in; the Newman sample imports with no warnings and its
  translated tests pass.

## Script notes (P9.3)
- New crate `snapreq-script` (ADR-031): `run(code, Input, Limits)`, `run_chain`; the API is `src/prelude.js`
  (pure JS over a JSON input/output document; no host functions, no loader). Limits 2 s / 32 MiB / 512 KiB.
- Format: `ScriptSpec` (`[script]` pre_request/_file, test/_file) on collection, folder, request files and flow
  steps; validated on load. `snapreq-test::script` (`Scripts::for_request`, `script_request`, `apply_request`,
  `outcomes`, `run_async`); `Session::send_spec_inner` runs pre-request scripts before building (then rebuilds
  scopes) and test scripts after captures; `RequestRun.logs`; `SendSpec.script` for flow steps.
- CLI `send` runs scripts too (`collection_cmd::build` → `TestScripts`); `run` prints `log:` lines with -v or on
  failure. Server exchange JSON and reports carry `logs`. UI: Scripts section in the request builder, "Script
  output" in the Checks tab; scope and draft models keep `[script]`.

## Export notes (P9.2)
- `snapreq-interop::export` (`Target`, `snippet(req, target, named_secrets)`): a `Spec` (URL with query, headers
  incl. auth headers, basic/digest/aws, body) rendered per target; secret values become `Part::Env` references to
  `SNAPREQ_SECRET_<NAME>` (named from scopes, also `--var` values of declared secrets) or `SNAPREQ_SECRET_n`.
  `har_export` (`from_exchanges` for CLI runs, `from_history` for stored entries; credential headers redacted).
  ADR-030.
- CLI `send --export FORMAT` (`collection_cmd::build` now returns named secrets), `run --report har:FILE`.
  Server `export.rs`: `POST /api/v1/requests/export`, `GET /api/v1/history/{id}/har`. UI: `ExportDialog`
  ("Code" button in the request builder, format remembered in localStorage), "Download HAR" in history entries.
- Generated Python/JS/Rust/shell were syntax-checked by hand (ast, node --check, rustfmt, bash -n); Go is not
  installed locally.

## Import notes (P9.1)
- New crate `snapreq-interop` (ADR-029): `detect`, `import(text, Options { format, name })`,
  `add_postman_environment`, `Imported { collection, items: Vec<Node>, environments, warnings }` with
  `write(dir, key)` (refuses non-empty dirs, writes order lists, stores secret values via `secrets::store`, loads
  the result back). Modules: `curl` (shell tokenizer), `postman`, `openapi` (refs, examples, security), `insomnia`
  (v4 JSON, v5 YAML, template tags), `har`, `hurl` (→ one flow), `util` (`protect_headers` moves credentials into
  secrets, `tidy_headers`, slugs, placeholders). `Imported::secret` avoids names the source uses (`reserve`).
- YAML through `serde-saphyr` (deserialize only). `save.rs` now writes `[capture]` entries inline.
- CLI `snapreq import FILE|- [--into DIR] [--format] [--name] [--env FILE]… [--json]` (`import_cmd.rs`; seals only
  with SNAPREQ_SECRET_KEY, like `secret set`). Server `POST /api/v1/collections/import` (`import.rs`, editors,
  32 MiB body limit, sealed, audited `collection.imported`). UI: `ImportDialog` (⇩ in the sidebar),
  `model/importing.ts`, `app.importCollection`.
- Fixtures in `crates/snapreq-interop/tests/fixtures` (Newman's public sample collection, unchanged, plus
  hand-written ones); the CLI test imports the Newman sample and runs it against a local echo server.

## Backup notes (P8.6)
- `backup.rs`: `create` (VACUUM INTO snapshot, manifest, config, workspace, key with `--with-key`),
  `restore` (staging inside the data dir, `unpack_in`, refuses links, `--force` to replace). CLI
  `snapreq backup FILE|-` / `snapreq restore FILE|-` (`user_cmd.rs`, `ServerDirs`). ADR-028.
- Phase 8 acceptance: docs/SECURITY-REVIEW.md (checklist with evidence, accepted risks).

## Hardening notes (P8.5)
- `security.rs`: `Limiter`/`Limits` (failures only: login per name 10 and per IP 50, unknown tokens per
  IP 50, 15 min), `too_many` (429 + Retry-After), `ClientIp` extension (TCP peer; serve uses
  `into_make_service_with_connect_info`), `headers` middleware, `host_allowed` (no-login loopback
  servers; `[auth] allowed_hosts`), audit (`Audit` builder, `Store::audit/audit_log/prune_audit`,
  migration 6, `GET /api/v1/audit`). `Identity.ip`. CLI `snapreq audit`; `snapreq user` ops audited.
  UI: "Recent activity" in the account dialog.

## Sealed secret notes (P8.4)
- `snapreq-collection::sealed` (SecretKey, seal/open AES-256-GCM with the name as AAD, `find`,
  `load_or_create`), `Secrets::load_with_key` / `locked()`, `secrets::store(…, key)`, `seal_all`.
- Server: `AppState.secret_key` (`SNAPREQ_SECRET_KEY` or `<data>/secret.key`), seals plain values in
  all workspaces at start, `api::load_secrets`. CLI: `collection_cmd::secret_key/load_secrets` (env or
  default data dir key; warns about locked values); `secret set` seals only with SNAPREQ_SECRET_KEY.
- Security fix found here: viewers may not send `file`/`vars`/`data` (could exfiltrate secrets);
  live response `url` hides secrets.

## OIDC notes (P8.3)
- `oidc.rs`: `[oidc]` config (`OidcConfig`, secret also from `SNAPREQ_OIDC_CLIENT_SECRET`), discovery
  cached, pending logins in memory (state → nonce + PKCE verifier, 10 min), `snapreq_oidc` cookie
  (Lax, path `/api/v1/auth/oidc`), `Store::oidc_user` (migration 5 `identities`; new accounts get
  password hash `!`, names get `-2` suffixes). ID token from the token endpoint, not signature-checked
  (ADR-025). Errors redirect to `/?login_error=…`, shown by `LoginView`.
- `tests/oidc.rs` has a mock provider (discovery, authorize redirect, token endpoint checking PKCE).

## Workspace notes (P8.2)
- `workspaces.rs`: `Role` (viewer < editor < owner), Store methods (workspaces, members, set_member keeps
  an owner, sole_ownerships), extractors `Space`/`EditSpace` (header `X-SnapReq-Workspace`, default
  `default`; 404 for non-members, 403 for low roles), `/api/v1/workspaces` routes. `collection_dir` and
  `load_collection` take a `&Space`. `default` = the workspace dir itself, others = subdirectories
  (ADR-024; the maintainer chose subdirectories and "any user creates workspaces"; existing
  collections were kept in place instead of moved, see the ADR).
- Migration 4: `workspaces`, `members`, `workspace` on history/runs. `create_user` makes the first
  account owner of `default`; `delete_user` refuses sole owners of shared workspaces.
- Execute/run resolve the workspace only for collection work (raw requests need none); WebSocket jobs
  read a `workspace` field; flow states keyed by `ws/collection`.
- UI: `state/workspaces.svelte.ts` (switch = localStorage + reload), header picker + ⚙
  `WorkspaceDialog` (rename, members, leave, delete, new), `NoWorkspace` page, read-only badge for
  viewers, env choices stored per workspace, history re-run only opens collection requests of the
  current workspace. `client.ts` sends the workspace header; `ws.ts` adds `workspace` to jobs.

## Accounts notes (P8.1)
- Server: `accounts.rs` (Store methods: users, Argon2id passwords, sessions and `snapreq_pat_` tokens
  stored as SHA-256), `auth.rs` (`guard` middleware → `Identity` extractor, `/api/v1/auth/*`), config
  `[auth] mode/session_days/secure_cookies`, `--auth`, `SNAPREQ_AUTH`. Migration 3 adds `users`,
  `sessions`, `api_tokens` and `user_id` on history/runs; every history/run Store method takes an
  `Owner` (`Option<i64>`, `NULL` = no accounts). `FlowStates` keyed by owner too.
- Defaults (ADR-023, amends ADR-018): `accounts` on non-loopback binds, `none` on loopback;
  `allow_remote` now means "no login on a non-loopback bind" (the Docker image keeps it, so it is
  unchanged; `SNAPREQ_AUTH=accounts` + `docker exec … snapreq user add` turns logins on).
- CSRF: cookie-authenticated non-GET requests and the login need `X-SnapReq-CSRF`; WebSocket needs a
  same-origin `Origin` and re-checks its session/token before each job.
- CLI: `snapreq user add|list|passwd|remove` (`user_cmd.rs`), password from hidden prompt (twice) or stdin.
- UI: `Root.svelte` gates on `auth.check()`; `LoginView`, `AccountDialog` (tokens, password, log out),
  `state/auth.svelte.ts`; `client.ts` sends the CSRF header and calls `onUnauthorized` on 401; the
  WebSocket client fails pending jobs on an `unauthorized` error. `scripts/web-e2e.sh` starts a second
  server with accounts (port 7882, users ana/bo) for `e2e/accounts.spec.ts`.

## Web UI notes (P7.8)
- Themes: `state/theme.svelte.ts` (system/light/dark, localStorage, applied in `main.ts` before mount;
  no inline script because of the CSP). Contrast fixed where axe found it (light `--warn`, `--ok`,
  `--m-get`, `--m-post`; dark `--muted`; `--primary-bg` for primary buttons).
- Shortcuts: `state/shortcuts.ts` (`match` by `code` for Alt combos, one list for handler and the `?`
  dialog); CodeMirror's Mod-Enter binding removed (it inserted a line as the request was sent).
- Accessibility: skip button, `main` landmark, open tabs as a navigation list with `aria-current`,
  accessible names that contain the visible text, `web/e2e/a11y.spec.ts` runs axe (WCAG 2.1 A/AA) over the
  main screens in both themes. `@axe-core/playwright` is MPL-2.0 (test-only, not shipped).
- Phase 7 acceptance review against docs/CLI.md: added proxy, CA/client certificates, TLS versions and
  host pinning (`--resolve`, scratch requests) to the settings editor, and `insecure`/`timeout_ms` to
  runs. Not in the UI: `--secret` (use secret variables), the OAuth authorization-code grant (needs a
  browser on the server machine). docs/WEB.md is the user guide; README updated.

## Web UI notes (P7.7)
- snapreq-test: `SessionState` (captured values + hidden secrets; `Session::state/restore`),
  `FlowSteps { from, to, start }` on `Plan::flow_steps`, `run_flow_steps`; `FlowRun.range/states/planned()`.
- Server: `RunBody.capture_wire/from_step/to_step/resume`; `flow_states::FlowStates` (in memory, last 50
  runs, 1 h, never stored or returned); run responses add `resumable_steps` and `range`. Migration 2 adds
  `runs.junit`/`runs.html` (rendered at run time); `GET /runs/{id}/report/{json,junit,html}` (attachment,
  CSP sandbox).
- UI: `RunnerTab`/`RunnerView` (env, iterations, CSV/JSON data, parallel, delay, bail, vars; live progress
  from WebSocket progress events; report fetched from `GET /runs/{id}`), `ReportView` + `StepDetail`
  (response, checks, until, captures, debug), `RunsPanel` (sidebar Runs), `RunTab`, `FlowTab`/`FlowView`
  (step list with drag/↑↓, `StepEditor` reusing the request editors, Run all / Step / ▶ run from step N
  using `resumeRun[step]` → run id, "variables after step N" from captures), `model/flow.ts`,
  `model/runs.ts` (`parseData`, `stepResponse`, `capturedAfter`).

## Web UI notes (P7.6)
- Server: history filters `class` (status class), `failed`, `collection`; `DELETE /history/{id}` and
  `DELETE /history` (`Store::delete_history`).
- UI: sidebar switch Collections | History; `HistoryPanel` (debounced filters, `VirtualList` with
  `onend` paging, select two to compare), `HistoryView` (stored request + `ResultView`), `DiffView`
  (`@codemirror/merge`, JSON pretty-printed, headers sorted, timing table). `ResultView` is the response
  display shared by requests, history (and runs). Re-run: collection requests are opened and sent as
  they are now; other requests are rebuilt without credentials (`draftFromHistory`), with a notice.
- GitHub macOS test jobs since 4d1e74d were cancelled after 15 min without running a step (no runner
  picked them up); Ubuntu/Windows pass. Not a code failure; check the repository's Actions/billing.

## Web UI notes (P7.5)
- Server `tree.rs`: `GET/PUT/DELETE /collections/{id}/folders/{key}` (folder.toml; PUT creates the dir),
  `POST /collections/{id}/move` (`kind`, `from`, `to`, `name`, `place.before` → rename on disk + parent
  `order` rewritten; rename in place keeps the position), `PUT /collections/{id}/secrets/{env|shared}/{name}`
  (`secrets::store`, never returned). Deleting a request or folder drops it from the parent's `order`.
- UI: `state/tabs.svelte.ts` (`RequestTab`, `ScopeTab` for collection/folder settings, `EnvTab`),
  `state/prompt.svelte.ts` + `PromptDialog` (promise dialogs), `state/tree.svelte.ts` (expanded nodes,
  drag item, `canDrop`), `ItemMenu` (menu button), `CollectionTree` (HTML5 drag and drop: top 30% of a
  row = before it, rest of a folder row = into it; collection row = root), menu Move up/down/to folder as
  the keyboard alternative, `VarsEditor`, `ScopeView`, `EnvView`. Open tabs follow moved items.

## Web UI notes (P7.4)
- `model/trace.ts`: `hops()` (same rules as the CLI waterfall: split at redirect/auth challenge, segment =
  [at − duration, at]; per hop request/response/DNS/connection/reuse/TLS/wire), `describe()`, `daysLeft()`.
- `DebugView` (Waterfall · Events · Connection/Redirects · TLS · Wire log, hop selector), live from
  `tab.live` while sending; failures show `error.trace`. Wire capture is HTTP/1.1 only (engine).
- Component tests run in jsdom (`// @vitest-environment jsdom`, `resolve.conditions: ["browser"]` under
  Vitest). E2E: the server pools connections across tests, so DNS/connect assertions use `localhost`.

## Web UI notes (P7.3)
- `BodyViewer` (Pretty/Raw/Preview/Hex; CodeMirror read-only with search panel, pretty-print up to 8 MiB,
  HTML preview in `<iframe sandbox="">` srcdoc, images via blob URLs, download/copy), `HexView` on
  `VirtualList` (fixed-height rows), `HeadersTable`, `CookiesTable` (`parseSetCookie`), `ChecksView`.
- `model/http.ts` (status texts, phases, sizes, cookies, text detection, hex rows), `model/format.ts`.
- mock_shop gained `/catalog?items=N` (with Set-Cookie), `/page` (HTML), `/old-health` (302) for e2e.

## Web UI notes (P7.2)
- Server: `ExecuteBody.file` (unsaved request file sent in its collection context via
  `Collection::put_request`, in memory) and `capture_wire`; `GET /collections/{id}/variables?key=&env=`
  (`Scopes::visible`, secret values never returned).
- `web/src/lib/api/types.ts`: hand-written core/file/report types; `ws.ts` `JobClient` (one lazy
  WebSocket, jobs routed by id, pending jobs fail on close, reconnects).
- `model/draft.ts`: `Draft` ⇄ request file (unedited keys kept in `rest`; rows with the same name grouped)
  and → core request (scratch requests). `model/vars.ts`: placeholders and their state.
- `editor/cm.ts`: CodeMirror setup; `{{var}}` decorations (`setVars` effect), hover description,
  completion. `state/app.svelte.ts`: `AppState` (collections, env per collection in localStorage, tabs,
  send via WebSocket with `capture_wire`, save / save as); `state/route.ts` hash routes.
- E2E: `scripts/web-e2e.sh` (mock shop on 8765 + `snapreq serve` on a copy of examples/shop, Playwright
  Chromium, `web/e2e/`); in check.sh when the Playwright browser is installed; CI job `web-e2e`.

## Web UI notes (P7.1)
- `web/`: Svelte 5 + TS 5.9 (openapi-typescript needs TS 5; TS 7 is out) + Vite 8 + Vitest 5. Scripts: `dev`
  (proxy `/api` + ws and `/healthz` to :7878, `SNAPREQ_DEV_SERVER` overrides), `build` (→ `web/dist`), `check`
  (svelte-check, warnings fail), `test`, `gen:api`.
- Typed client: `web/openapi.json` (committed; server test `the_web_ui_copy_is_up_to_date`,
  `SNAPREQ_UPDATE_OPENAPI=1` rewrites) → `src/lib/api/schema.d.ts` (`--empty-objects-unknown`, CI checks it is
  current) → `src/lib/api/client.ts` (`createApi`, `unwrap` → `ApiError {status, kind}`, `serverVersion`).
- `snapreq_server::ui` (ADR-021): rust-embed of `web/dist` (`allow_missing`; debug builds read from disk), router
  `.fallback(ui::serve)`; `respond(assets, …)` testable with fake files; SPA fallback for extensionless paths,
  unknown `/api/…` → JSON 404, ETag/304, `assets/` immutable, CSP + `X-Frame-Options` on HTML; "not built" page.
- Dockerfile has a `node:24-bookworm-slim` stage; CI jobs `web` and `docker` (smoke test also checks the UI).
- npm audit: advisories only in build tooling (deepmerge via vite-plugin-svelte, minimatch via openapi-typescript).

## Docker notes (P6.7)
- `Dockerfile`: rust:1-bookworm builder (cache mounts; removes rust-toolchain.toml) → `gcr.io/distroless/cc-debian12:nonroot`,
  `SNAPREQ_BIND=0.0.0.0:7878`, `SNAPREQ_ALLOW_REMOTE=1` (so `block_metadata` is on), `/data` volume owned by 65532,
  HEALTHCHECK via `snapreq send --fail` (ignored by podman's OCI format). `docker-compose.yml` publishes on 127.0.0.1.
- `scripts/docker-smoke.py` (stdlib only, includes a tiny WebSocket client): health, REST execute, WebSocket execute
  with live traces, history, metadata endpoint blocked. Passed against the image built with podman.

## Outbound policy notes (P6.6)
- Core `ConnectPolicy` (`ClientConfig::connect_policy`), checked in `conn::connect` via `allowed()` on every resolved
  address (direct, proxy address, locally resolved SOCKS5 target) and by name for proxy-resolved targets; blocked
  addresses skipped with an `info` event; none left → connect-phase failure naming the reason (ADR-020).
- `snapreq_server::outbound`: `Rule` (CIDR/IP/host/`*.domain`, own CIDR math, IPv4-mapped normalised),
  `OutboundPolicy { allow, deny, block_private, block_metadata }` (deny > allow > metadata > private > allow-list),
  `[outbound]` config; `block_metadata` defaults to `allow_remote`. The server's `Client` gets the policy;
  warning when remote with an open policy.

## OpenAPI notes (P6.5)
- `utoipa` 6 + `utoipa-axum` 0.3: `api::router()` is an `OpenApiRouter` built with `routes!(...)` from
  `#[utoipa::path]`-annotated handlers, so routes and docs cannot drift; `lib::routes()` splits router and document
  and rewrites `{*key}` to `{key}`. `snapreq_server::openapi()`; served at `/api/v1/openapi.json` (version filled in).
  Core types are `value_type = Object` (utoipa stays out of snapreq-core). Test checks every method+path.

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
| 6 Self-hosted server | ✅ done |
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
- P6.5 — 2026-11-03 — OpenAPI 3.1 description generated from the handlers (utoipa), served at /api/v1/openapi.json. 332 tests total.
- P6.6 — 2026-11-03 — Outbound request policy (SSRF): connect-time checks of every address in the engine, server allow/deny lists, private and cloud-metadata blocking (ADR-020). 339 tests total.
- P6.7 — 2026-11-03 — Docker image (distroless, non-root), docker-compose example, smoke test passing against the image. Phase 6 complete.
- P7.1 — 2026-11-03 — Web UI setup: Svelte 5 + TS + Vite in `web/`, typed API client generated from the OpenAPI document, UI embedded in the binary (ADR-021), Docker/CI updated. 345 Rust + 3 web tests.
- P7.2 — 2026-11-03 — Request builder: method, URL with variable highlighting, params, headers, body editors (CodeMirror), auth, settings, checks; unsaved edits sent in collection context; save / save as; Playwright e2e tests.
- P7.3 — 2026-11-03 — Response viewer: pretty/raw/preview/hex, headers, cookies, checks, search in the body, virtualized large bodies.
- P7.4 — 2026-11-03 — Debug view: live timing waterfall per redirect hop, event list with headers, connection and redirect chain, TLS and certificate chain, raw wire log (text/hex).
- P7.5 — 2026-11-03 — Collections sidebar: tree with drag and drop and menus (create, rename, duplicate, move, delete), collection/folder settings, environment editor with secret values, environment switcher.
- P7.6 — 2026-11-03 — History: search and filters, stored exchange view, re-run (without stored credentials), compare two responses (body/headers diff, timing).
- P7.7 — 2026-11-03 — Runner (iterations, data rows, live progress, report with per-step debug, HTML/JUnit/JSON downloads), runs list, flow editor with step-by-step runs, captured variables after each step, re-run from step N.
- P7.8 — 2026-11-03 — Light/dark/system themes, keyboard shortcuts with help dialog, accessibility pass (axe WCAG 2.1 AA in both themes), CLI parity review (proxy, TLS, resolve, run -k/-t). Phase 7 complete. 352 Rust + 40 unit + 30 browser tests.
- P10.3 — 2026-11-06 — GraphQL: `graphql` body type (core, files, `send --graphql`), `snapreq introspect` and `POST /requests/graphql-schema` (SDL), schema-aware query editor and schema browser in the UI. ADR-035.
- P10.2 — 2026-11-06 — Server-Sent Events: streaming body observer and stop in core, `snapreq sse`, live events and cancel over the server WebSocket, Events tab and Stop button in the UI. ADR-034.
- P10.1 — 2026-11-06 — WebSocket client: core handshake through the engine, `[websocket]` in request files, `snapreq ws`, server-relayed sessions, message log in the UI. ADR-033.
- P9.4 — 2026-11-06 — Postman pre-request/test scripts translated to `[script]` during import (`pm.*` and legacy API rewritten, untranslatable lines commented with warnings, result compile-checked). ADR-032. The Newman sample imports and its tests pass. **Phase 9 complete.**
- P9.3 — 2026-11-04 — Pre-request and test scripts in JavaScript (QuickJS sandbox, `snapreq.*` API with Jest/Chai `expect`), in send, run, flows, the server and the UI. ADR-031.
- P9.2 — 2026-11-04 — Export: `send --export curl|python|javascript|go|rust`, `run --report har:FILE`, API export and history HAR, "Code" dialog and HAR download in the UI; secrets become environment variable references. ADR-030.
- P9.1 — 2026-11-04 — Import from cURL, Postman v2.1 (+ environments), OpenAPI 3.x, Insomnia v4/v5, HAR and Hurl (as flows); CLI, API and UI; credentials become secret variables. ADR-029.
- P8.6 — 2026-11-04 — Backup and restore (`snapreq backup|restore`, consistent snapshot while serving, key excluded by default, safe unpacking). ADR-028. Security review checklist completed (docs/SECURITY-REVIEW.md). **Phase 8 complete.**
- P8.5 — 2026-11-04 — Audit log (API, CLI, UI), failure rate limits, security headers, Host allowlist against DNS rebinding. ADR-027.
- P8.4 — 2026-11-04 — Secret values sealed at rest (AES-256-GCM, key from env or data dir), existing plain values sealed at start, CLI opens them; viewers limited to saved requests without overrides. ADR-026.
- P8.3 — 2026-11-04 — OIDC login (code flow + PKCE, state cookie, nonce), automatic accounts with optional verified-domain allowlist, "Log in with …" button. ADR-025.
- P8.2 — 2026-11-04 — Workspaces with owner/editor/viewer roles: directories under the workspace dir (`default` = the dir itself), members and roles in SQLite, `X-SnapReq-Workspace` header, workspace picker and dialog in the UI. ADR-024. Cross-user isolation and role rules tested in Rust and Playwright.
- P8.1 — 2026-11-04 — Local accounts (Argon2id), session cookies, API tokens, `none`/`accounts` auth modes (remote binds default to accounts), per-user history/runs/flow states, CSRF header and WebSocket origin checks, `snapreq user` commands, login page and account dialog in the UI. ADR-023. Two-user isolation tested in Rust and Playwright.

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
- **2026-11-03 (17):** Phase 6 complete (P6.3–P6.7). Docker CI job still to add. Session ended at the usage limit.
- **2026-11-03 (18):** Added the Docker smoke-test CI job (da78f47). P7.1 done: web project, typed client, embedded UI, verified with a release binary and the podman-built image.
- **2026-11-03 (19):** Phase 7 complete (P7.2–P7.8), each checkpoint pushed after scripts/check.sh passed. Added Playwright end-to-end tests (mock shop + real server) and axe audits; ADR-021, ADR-022. GitHub macOS test jobs were cancelled before starting for 4 commits (runner side), fine again from 3bcc73f. The web-e2e CI job failed for P7.7/P7.8: a race in the flow test (old step results looked current while a new run was going), reproduced with `CI=1 taskset -c 0`; fixed by clearing results that a run replaces. Playwright's `github` reporter now turns e2e failures into job annotations.
- **2026-11-03 (20):** Maintainer testing: the method field (a text box with suggestions) is now a fixed dropdown (`MethodSelect`) in the request builder and flow steps.
- **2026-11-03 (21):** macOS runners were often unavailable for pushes (jobs queued, then cancelled without running). Maintainer chose: macOS tests run nightly (`.github/workflows/nightly.yml`, 02:17 UTC, also by hand); per-push CI tests Ubuntu and Windows.
- **2026-11-04 (22):** P8.1 done: accounts, sessions, API tokens, per-user history and runs, CSRF and WebSocket origin checks, `snapreq user`, UI login and account dialog; ADR-023 (remote binds default to accounts, `--allow-remote` keeps no-login for trusted networks and the Docker image). scripts/check.sh and the full Playwright suite pass.
- **2026-11-04 (23):** P8.2 done: workspaces with roles (maintainer chose subdirectory layout and "any user creates workspaces"; existing collections stay in place as `default`), ADR-024. Stopped before P8.3 for the OIDC account-provisioning decision.
- **2026-11-04 (24):** Phase 8 complete (P8.3–P8.6 after the maintainer chose automatic OIDC accounts with a domain allowlist). Found and fixed a viewer secret-exfiltration path in P8.4. ADR-025–028, docs/SECURITY-REVIEW.md. Stopped for the maintainer's report before Phase 9.
- **2026-11-04 (25):** P9.1 done (maintainer: "start with phase 9"): `snapreq-interop` crate, `snapreq import`, `POST /api/v1/collections/import`, Import dialog. The public Newman sample collection imports and runs. 423 Rust tests, 45 unit tests and 35 Playwright tests pass.
- **2026-11-04 (26):** P9.2 done: export to cURL, Python, JavaScript, Go and Rust (secrets read from `SNAPREQ_SECRET_*`), HAR from runs and history. 431 Rust tests, 46 unit tests and 37 Playwright tests pass.
- **2026-11-04 (27):** P9.3 done: `snapreq-script` (rquickjs) with sandbox tests (loops, memory, stack, no modules or host APIs), `[script]` in the format, scripts in sessions, CLI send/run, server and UI. 439 Rust tests, 47 unit tests and 38 Playwright tests pass.
- **2026-11-06 (28):** P9.4 done, which completes Phase 9: Postman pre-request/test scripts are translated during import (ADR-032); the Newman sample imports without warnings and its tests pass. 443 Rust tests pass. Stopped for the maintainer's report before Phase 10.
- **2026-11-06 (29):** P10.1 done: WebSocket client in core, CLI, server and UI. 456 Rust tests, 50 unit tests and 39 Playwright tests pass.
- **2026-11-06 (30):** P10.2 done: SSE parser and streaming in core, CLI, server and UI. 466 Rust tests, 51 unit tests and 40 Playwright tests pass.
- **2026-11-06 (31):** P10.3 done: GraphQL bodies, introspection in CLI and server, schema-aware editor in the UI. 472 Rust tests, 53 unit tests and 41 Playwright tests pass.

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

## ADR-016: Assertion format and libraries
**Status:** Accepted (2026-11-03)
**Context:** P5.1 needs declarative checks in request files that read well, diff cleanly, can be edited by the
web UI and work unchanged inside flow steps (P5.3, `[step.assert]`). Alternatives considered: a list of
`[[assert]]` tables (one check each: verbose), and Bruno-style strings (`"res.status" = "eq 200"`: compact but
an extra mini-language with its own escaping).
**Decision:** An `[assert]` table with fixed subjects (`status`, `duration_ms`, `body`, `schema`) and two keyed
sub-tables, `[assert.headers]` and `[assert.json]` (JSONPath keys), mirroring `[headers]`. A plain value means
`equals`; a table lists operators, all of which must pass. Evaluation lives in `snapreq-test`; the format in
`snapreq-collection`. Libraries: `serde_json_path` (RFC 9535 JSONPath, MIT), `regex` (MIT/Apache-2.0),
`jsonschema` without default features (MIT; no HTTP fetching of remote `$ref`s, no extra TLS stack).
**Consequences:** One key per subject, so two checks on the same JSONPath go in one table. Comparing with an
object needs `{ equals = { ... } }`. Remote schema references are not fetched (local files only).

## ADR-017: Flows as `*.flow.toml` files inside a collection
**Status:** Accepted (2026-11-03). Requested by the maintainer ("chain testing", like Hurl, in the CLI and web UI).
**Context:** Workflow tests chain requests: log in, create, poll, clean up, passing values along. Hurl does this
with its own text format in one file. SnapRequest already has TOML request files, inheritance, environments,
secrets, `[assert]` and `[capture]`.
**Decision:** A flow is a `*.flow.toml` file anywhere inside a collection (so it shares environments, secrets, auth
and folder inheritance), excluded from the request tree and addressed by key. Ordered `[[step]]` tables either
reference a collection request (`request = "key"`) or define one inline with the request-file keys; steps add
`vars`, `[step.capture]`, `[step.assert]`, `delay_ms` and `[step.retry]` (`max`, `every_ms`, optional `until`
checks). Variable precedence: `--var` > step vars > captured values > flow vars > the request's scopes. Runs stop at
the first failing step. The engine is `snapreq_test::{Session, run_flow}`, which the CLI (`snapreq run`), the
server and the web UI (P7.7) share.
**Consequences:** One format and parser for everything, editable by the web UI with `toml_edit`. Long flows are
wordier than Hurl files; a Hurl importer (P9.1) eases migration. `request` steps cannot patch the referenced
request (keeps steps and files from drifting); inline steps cover one-off variations.

## ADR-018: The server refuses non-loopback addresses until it has authentication
**Status:** Accepted (2026-11-03)
**Context:** `snapreq serve` executes HTTP requests on behalf of its callers. Until authentication (P8.1) and the
outbound request policy (P6.6) exist, anyone who can reach the server could use it as an open proxy into the
network it runs in (internal services, cloud metadata endpoints).
**Decision:** The default bind is `127.0.0.1:7878`. Binding to any non-loopback address fails with an explanation
unless `--allow-remote` / `SNAPREQ_ALLOW_REMOTE=1` / `allow_remote = true` is set; when it is, a warning is
logged. Configuration precedence: flags > `SNAPREQ_*` environment variables > config file > defaults.
**Consequences:** Safe by default for local use. Containers (P6.7) must set `SNAPREQ_ALLOW_REMOTE=1` and bind
`0.0.0.0` inside the container while publishing the port on the host's loopback. Revisit once P8.1 lands:
remote binds should then require authentication instead of a flag. **Amended by ADR-023.**

## ADR-019: rusqlite instead of sqlx; MSRV 1.89 (amends ADR-006 and ADR-012)
**Status:** Accepted (2026-11-03), chosen by the maintainer.
**Context:** ADR-006 picked `sqlx` for SQLite. `sqlx` 0.9 needs Rust 1.94 and brings an async driver, a macro
layer and a larger dependency tree; the server's storage needs are small (append history and run results, list,
prune) and a single process owns the database.
**Decision:** Use `rusqlite` with the `bundled` feature (SQLite compiled in, no system library), called from
`tokio::task::spawn_blocking` behind a small `Store` type. Migrations are plain SQL steps tracked with SQLite's
`PRAGMA user_version` (no migration crate). The MSRV becomes 1.89, which `rusqlite` 0.41 requires and the MSRV
policy of ADR-012 allows.
**Consequences:** Fewer dependencies and no compile-time database checks; queries are covered by tests instead.
A C compiler is needed to build (available on all CI runners). Postgres, if ever needed, goes behind the `Store`
interface.

## ADR-020: The outbound request policy is checked at connect time, in the engine
**Status:** Accepted (2026-11-03)
**Context:** A server that executes requests for its users is an SSRF vector: users can reach internal services
and cloud metadata endpoints. Checking URLs before sending is not enough: a public name can resolve to an internal
address (or change between check and connect, DNS rebinding), redirects lead elsewhere, and hosts can be pinned.
**Decision:** `snapreq-core` exposes a `ConnectPolicy` trait on `ClientConfig`; the engine calls it for every
resolved address right before connecting (direct connections, proxies, locally resolved SOCKS5 targets) and for
proxy-resolved targets by name. Blocked addresses are skipped, and the request fails only if nothing is left.
The server implements it as `OutboundPolicy` from `[outbound]` (deny, allow, `block_private`, `block_metadata`),
with `block_metadata` on by default whenever `allow_remote` is on (PLAN P6.6).
**Consequences:** No check can be bypassed by DNS or redirects. The CLI keeps an open policy (it runs as its user).
Requests through an HTTP or SOCKS5h proxy rely on name rules for the target, because the proxy resolves it.

## ADR-021: Web UI build, embedding and typed client
**Status:** Accepted (2026-11-03)
**Context:** The UI (ADR-005) must ship inside the single binary, but Rust-only contributors and `cargo test`
should not need Node.js. The UI's API client should not drift from the server's handlers.
**Decision:** Vite builds `web/` to `web/dist`, which `snapreq-server` embeds with `rust-embed`
(`allow_missing`: without a build the server serves a page explaining how to build the UI; debug builds read the
files from disk, so `npm run build` is picked up without recompiling). The server's router falls back to the UI
for every unmatched `GET` (SPA routes get `index.html`, unknown `/api/…` stays a JSON 404); hashed `assets/` are
cached as immutable, pages carry a strict CSP. `web/openapi.json` is a committed copy of the generated OpenAPI
document, checked by a server test (`SNAPREQ_UPDATE_OPENAPI=1` rewrites it); `openapi-typescript` generates
`web/src/lib/api/schema.d.ts` from it (CI checks it is current) and `openapi-fetch` is the typed client.
**Consequences:** Release binaries and the Docker image need `npm run build` first (the Dockerfile has a Node
stage). Core types are `Object` in the OpenAPI document, so their TypeScript shapes are written by hand in the UI.
Publishing `snapreq-server` to crates.io will need the built files inside the crate (P11.2). npm audit reports
advisories only in build-time tooling (vite-plugin-svelte's `deepmerge`, openapi-typescript's `minimatch`), none
of which ships in the bundle.

## ADR-022: Step-by-step flow runs resume from states kept in server memory
**Status:** Accepted (2026-11-03)
**Context:** The flow editor (PLAN P7.7) runs a flow step by step and re-runs from step N. Step N needs the
values the earlier steps captured, including secret ones (a login token). Run reports hide secrets, so the
browser cannot hand them back, and storing them would put credentials on disk.
**Decision:** `snapreq-test` records the session state (captured values and the secrets to hide) before
each flow step (`FlowRun::states`, `Session::state/restore`, `FlowSteps`). The server keeps the states of
the last 50 single-iteration flow runs in memory for an hour (`flow_states.rs`), keyed by run id, and a run
with `from_step` + `resume: <run id>` starts from the stored state. States are never stored or returned; a
response only lists the steps that can be resumed.
**Consequences:** Resuming works across edits of later steps and keeps secrets hidden as in a full run.
States are lost when the server restarts or after an hour (a 409 asks to run from the start). With
multiple users (Phase 8) states must be scoped to their owner.

## ADR-023: Local accounts, hashed sessions and API tokens; remote binds default to accounts (amends ADR-018)
**Status:** Accepted (2026-11-04)
**Context:** PLAN P8.1 needs local accounts, sessions and API tokens, and a single-user mode without a login
for localhost. ADR-018 refused non-loopback binds until authentication existed, with `--allow-remote` as the
escape hatch, which the Docker image sets. The UI talks to the API with `fetch` and a WebSocket.
**Decision:** `auth.mode` is `none` or `accounts` (`--auth`, `SNAPREQ_AUTH`, `[auth] mode`). The default is
`accounts` on a non-loopback address and `none` otherwise; `none` on a non-loopback address still needs
`allow_remote`, which now means "trusted network, no login" (so the Docker image keeps working as before).
Passwords are Argon2id (`argon2` crate, default parameters, hashed off the async threads; unknown users are
verified against a dummy hash). Sessions are an HttpOnly `SameSite=Strict` cookie, API tokens a
`snapreq_pat_` bearer token; both are 256 random bits from `ring` and stored only as SHA-256 digests. One
axum middleware identifies the caller for every `/api` route. Cookie-authenticated requests that change
something, and the login, must carry `X-SnapReq-CSRF` (a custom header forces a CORS preflight, which the
server never grants; `SameSite` alone does not cover other ports of the same host). The WebSocket handshake
must have this server's `Origin`, and the socket re-checks its session or token before each job. History,
runs and flow states carry an owner (`NULL` without accounts) and every query filters by it; collections stay
shared until workspaces (P8.2). Accounts are managed with `snapreq user …` on the server's database; there is
no sign-up and no admin role yet (P8.2).
**Consequences:** Exposing the server no longer needs a flag, only a user. Entries made without accounts are
not visible to accounts later (and the reverse). Login rate limiting, security headers and an audit log
remain for P8.5; DNS-rebinding protection for a no-login loopback server (a `Host` check) as well.

## ADR-024: Workspaces are directories; the API selects one with a header
**Status:** Accepted (2026-11-04); layout and permissions chosen by the maintainer.
**Context:** PLAN P8.2 adds workspaces with owner, editor and viewer roles. Collections are directories in the
workspace directory and should stay hand-editable and git-friendly. Every collection endpoint and about 30 UI
calls address collections as `/collections/{id}/…`.
**Decision:** A workspace is a directory: `default` is the workspace directory itself (existing collections stay
where they are, so a `--workspace` pointing at a git repository is not reorganised), every other workspace `id`
is `<workspace dir>/<id>/`. Workspace ids and collection ids of `default` share one namespace, checked on
creation. Membership and roles live in SQLite (`workspaces`, `members`, migration 4). Any user may create a
workspace and becomes its owner; owners add existing users; a workspace always keeps an owner. The first account
owns `default` (accounts that existed before the migration: the first owns it, the others edit it). The API keeps
its paths and picks the workspace with the `X-SnapReq-Workspace` header (default `default`); WebSocket jobs carry
a `workspace` field. Handlers get the workspace through extractors: `Space` (any member) and `EditSpace` (editor
or owner); the collection path helpers take a `Space`, so a handler cannot reach collection files without one.
Non-members get 404, too-low roles 403. Without accounts the single user owns every workspace.
**Consequences:** No route or UI call changed shape; scripts add one header. History and runs record their
workspace. The UI reloads when switching workspaces. Ownerless workspaces (all members deleted) keep their files
but no one can open them until P8.6 tooling or a database edit.

## ADR-025: OpenID Connect login with automatic accounts and no JWT library
**Status:** Accepted (2026-11-04); automatic accounts with an optional domain allowlist chosen by the maintainer.
**Context:** PLAN P8.3 adds login through OIDC providers. Verifying ID token signatures needs JWKS fetching and
RSA/EC JWT verification (another dependency tree), and accounts must come from somewhere.
**Decision:** Authorization code flow with PKCE (`S256`), `state` kept in server memory (10 minutes) and bound to
the browser by an HttpOnly `SameSite=Lax` cookie (Lax because the provider redirects back cross-site), and a
`nonce`. The ID token is taken only from the token endpoint response, so, as OIDC Core 3.1.3.7 allows, TLS to the
provider replaces the signature check; `iss`, `aud`, `exp` (60 s skew) and `nonce` are checked. The provider is
reached with a client outside the outbound policy (the administrator configured it). An identity (`iss`, `sub`)
is linked to one account in `identities` (migration 5); its first login creates the account (no password, no
workspace, unless it is the very first account); a taken name gets a numeric suffix, so an identity never links
to an existing account by name. `allowed_domains` admits only `email_verified` addresses of those domains.
**Consequences:** No new dependencies. The issuer should be `https://`; a plain-HTTP issuer (allowed for local
testing) would let a network attacker forge logins. Linking an existing local account to an identity, and
signature checks for front-channel tokens, are not supported.

## ADR-026: Secret values sealed in place with AES-256-GCM
**Status:** Accepted (2026-11-04)
**Context:** PLAN P8.4 asks for encrypted secret storage on the server. Secret values are kept in
`<collection>/.secrets/*.toml`, which the CLI also reads, and backups copy the data directory.
**Decision:** Keep the files and their layout; seal values in place as `enc:v1:` + base64url(nonce ‖ AES-256-GCM
ciphertext), with the variable name as associated data (`ring`, already a dependency). `snapreq-collection`
opens sealed values when loading secrets (`Secrets::load_with_key`); values it cannot open count as unset and are
listed (`Secrets::locked`). The server seals everything it writes and, at start, every plain value in the
workspace. The key comes from `SNAPREQ_SECRET_KEY` or `<data dir>/secret.key` (created with mode 0600); the CLI
looks in the same places. Found in passing: viewers could read secrets by sending unsaved edits or variable
overrides to their own host, so viewers now send saved requests only, and the final URL of a live response hides
secret values like the trace does.
**Consequences:** A copy of the workspace or the data directory without the key reveals no secret values. The key
must be backed up separately (P8.6 says so). Key rotation is not supported yet (re-seal with a new key would be a
small CLI command).

## ADR-027: Hardening: in-memory failure limits, header middleware, Host allowlist, SQLite audit log
**Status:** Accepted (2026-11-04)
**Context:** PLAN P8.5: audit log, rate limiting, CSRF protection, security headers; ADR-023 left login limits
and DNS rebinding for this checkpoint.
**Decision:** Rate limits count *failures* only (logins per user name and per client address, unknown tokens per
address) in fixed 15-minute windows in memory; legitimate use is never slowed, and a restart clears them. The
client address is the TCP peer (`X-Forwarded-For` is not trusted; behind a proxy every client shares the proxy's
address, so the per-name limit is the effective one). One middleware sets security headers without overriding
those a handler chose (report downloads keep `sandbox`). A no-login server on loopback (no `allow_remote`) answers
API requests only for loopback host names plus `auth.allowed_hosts`: with accounts, rebinding gains nothing
because the attacker's origin has no cookie. The audit log is a SQLite table (migration 6) without a foreign key
to users, so events outlive deleted accounts; it records actions and names, never values, and is pruned after a
year. Visibility follows ownership (own events, plus events in workspaces one owns); the server's command line
sees all, as there is no admin role.
**Consequences:** Anonymous failed-login events are only visible on the server machine. A distributed password
guess against many names from many addresses is slowed only by Argon2. Per-request rate limits for authenticated
users were not added (self-hosted, users are known).

## ADR-028: Backups are tar.gz archives with a VACUUM INTO snapshot; the key stays out by default
**Status:** Accepted (2026-11-04)
**Context:** PLAN P8.6: backup and restore of the full data directory. The database is written while the server
runs (WAL mode), and the secret key (ADR-026) protects the sealed values that a backup also contains.
**Decision:** `snapreq backup` writes `manifest.json`, `data/snapreq.db` (a `VACUUM INTO` snapshot, consistent while
serving), `data/config.toml` and `workspace/…` into a gzip tar (`tar` crate, MIT/Apache; `flate2` was already
used). The key is added only with `--with-key`, so by default a stolen backup reveals no secret values.
`snapreq restore` unpacks into a staging directory inside the data directory first (`unpack_in` refuses absolute
paths and `..`; links and special entries are refused), checks the manifest and database, then replaces the
database, config, key and workspace; non-empty targets need `--force`. Both are CLI commands on the server
machine: there is no admin role for an API endpoint, and Docker users run them with `docker exec`.
**Consequences:** One file to copy off-site; restores are all-or-nothing per archive. The key needs its own backup.

## ADR-029: Imports live in a `snapreq-interop` crate and move credentials into secret variables
**Status:** Accepted (2026-11-04)
**Context:** PLAN P9.1: import cURL, Postman v2.1 (with environments), OpenAPI 3.x, Insomnia, HAR and Hurl, from
the CLI and the web UI. Exported files and copied commands routinely contain live passwords, tokens and cookies,
and collections are meant to be committed to git.
**Decision:** A new library crate `snapreq-interop` (no I/O except writing the result; `thiserror`) parses each
format into an intermediate `Imported` collection, which writes ordinary collection files (`docs/FORMAT.md`) and
loads them back as a check. Shared by `snapreq import` and `POST /api/v1/collections/import`; P9.2 exports will
live there too. Choices:
- **Credentials:** literal values of passwords, bearer/basic `Authorization` headers, API keys, cookies,
  OAuth client secrets and AWS secret keys become secret variables (`{ secret = true }` in `snapreq.toml`) whose
  values go to `.secrets/` (sealed on the server, and by the CLI when `SNAPREQ_SECRET_KEY` is set). Values that
  already are placeholders (`{{token}}`) stay as they are; names the source uses are not taken (`password_2`).
- **Placeholders:** `{{name}}` is shared by Postman, Insomnia (`{{ _.name }}`), Hurl and cURL commands, and kept.
  Recorded or example text (HAR, OpenAPI examples) is literal, so `{{` is escaped there (`\{{`). Dynamic variables
  are mapped where an equivalent exists (`{{$guid}}`, `{% uuid %}`, `newUuid` → `{{$uuid}}`); the rest are warned
  about. Every import ends with a warning listing variables used but defined nowhere.
- **YAML** (OpenAPI, Insomnia v5) is read with `serde-saphyr` (MIT/Apache, MSRV 1.89, deserialize only): pure
  Rust, panic-free on hostile input, with resource budgets; `serde_yaml` is unmaintained.
- **OpenAPI:** one request per operation in a folder per first tag; servers become `base_url` (and environments
  when there are several); security schemes become collection or request auth backed by declared secrets;
  bodies and parameters get examples (`example`, `examples`, `default`, `enum`, or generated from the schema,
  stopping at a `$ref` that repeats). Swagger 2.0 is refused with a hint to convert it.
- **Hurl** files become one flow (inline steps, captures, asserts, retries); **HAR** skips static assets and uses
  `{{base_url}}` when there is one origin.
- Anything that cannot be represented (scripts, unsupported auth, xpath asserts, file paths that must be copied)
  is a warning in the result, never silently dropped. Inputs are capped at 64 MiB (32 MiB per API request).
**Consequences:** Imported collections are safe to commit as they are, but runs need the `.secrets/` values on the
machine that runs them (or `SNAPREQ_SECRET_*` variables). Postman scripts are not run: P9.3/P9.4 add scripting and
translate simple `pm.test` scripts.

## ADR-030: Exports read secrets from environment variables; HAR comes from CLI runs and history
**Status:** Accepted (2026-11-04)
**Context:** PLAN P9.2: export to cURL, HAR and code snippets (Python requests, JS fetch, Go, Rust reqwest). An
exported command is copied into chats, tickets and scripts, so it must not carry secret values, yet it should
run as it is.
**Decision:** `snapreq-interop::export` turns a built request (variables filled in, auth applied by meaning) into
cURL, Python, JavaScript, Go or Rust. Every secret value becomes a reference to `SNAPREQ_SECRET_<NAME>`, the
variable the CLI already reads secrets from (`${SNAPREQ_SECRET_TOKEN}`, `os.environ[...]`, `process.env...`,
`os.Getenv`, `std::env::var`); values without a known name get numbered variables, and any value that
would still appear is replaced by `<redacted>`. Basic auth is written as credentials (`-u`, `auth=`,
`SetBasicAuth`, …), never as a pre-encoded header. OAuth 2.0 reads `SNAPREQ_ACCESS_TOKEN`; digest and AWS
signing are kept where the tool has them (cURL, Python) and noted elsewhere. Settings are written only when
they differ from SnapRequest's defaults. The server builds the request exactly like `/requests/execute`
(`POST /api/v1/requests/export`), with the same viewer rule for overrides. HAR (`har_export`) is written
from exchanges this process made (`snapreq run --report har:FILE`) and from stored history
(`GET /api/v1/history/{id}/har`); credential headers are always `<redacted>`. Server-side run reports store
no request details, so they have no HAR form.
**Consequences:** Snippets run after exporting the secrets, the same way CI already provides them. HAR files are
safe to attach to tickets, but cannot replay authenticated requests as they are.

## ADR-031: Scripts in QuickJS (rquickjs) with a pure-JavaScript `snapreq` API
**Status:** Accepted (2026-11-04)
**Context:** PLAN P9.3: pre-request and test scripts in JavaScript via `rquickjs`, with a `snapreq.*` API
(variables, request, response, expect), time and memory limits and no file or network access; tests must prove
the sandbox. Scripts also have to work in `snapreq send`, `snapreq run`, flows and the server, and P9.4 will
translate Postman scripts into them.
**Decision:**
- A new crate `snapreq-script` wraps `rquickjs` 0.14 (MIT, QuickJS-ng, MSRV 1.87) with default features off: no
  module loader, no `std`/`os` modules, no host functions at all. The API is a JavaScript prelude over a JSON
  input document (variables, the request with placeholders, the response); the script's effects come back as a
  JSON output document (variables set, the changed request, tests, logs, an uncaught error). Nothing in the
  engine can call into Rust, so the sandbox is the language itself.
- Every script gets a fresh runtime: an interrupt handler enforces 2 s (uncatchable from JavaScript), plus a
  32 MiB memory limit and a 512 KiB stack limit. Engine globals the API relies on are captured before user code
  runs.
- Format: `[script]` with `pre_request`/`pre_request_file` and `test`/`test_file` in `snapreq.toml`,
  `folder.toml`, request files and flow steps; they run collection → folders → request → step. Pre-request
  scripts run before the request is built, so variables they set and `{{placeholders}}` they write are filled in
  like everything else. Test results are ordinary check outcomes (`test <name>`), so reports, JUnit, exit codes,
  retries and the UI need nothing new; variables set by scripts behave like captured values.
- `expect` speaks both Jest and Chai style, so translated Postman tests (P9.4) map almost one to one.
- Scripts run on a blocking thread (`spawn_blocking`) in sessions; `snapreq send` runs them in place.
**Consequences:** Scripts are deterministic and isolated, at the cost of no `fetch`, timers or `require`
(asynchronous tests are rejected). QuickJS is C code: an engine bug could be reachable from a script, which is
accepted because scripts come from editors (docs/SECURITY-REVIEW.md). Scratch requests in the UI (outside a
collection) have no scripts.


# SnapRequest server

`snapreq serve` runs the self-hosted server: a JSON API (this document) and the web UI at `/`
(built from `web/` and embedded into release binaries, ADR-021; every `GET` outside the API serves it).
Configuration (bind address, data directory, workspace, retention) is described in `docs/CLI.md`,
"Server".

- **Workspace:** the collections the API serves, one directory each (`<workspace>/<id>/snapreq.toml`).
  The directory name is the collection **id**. Files are the normal collection format, so they can be
  edited by hand, with the CLI, or kept in git.
- **Storage:** history and run results in `<data dir>/snapreq.db`, redacted (see `docs/CLI.md`).
- **Security:** no authentication yet (Phase 8). The server listens on loopback only unless
  `--allow-remote` is given (ADR-018).

## Docker

```sh
docker build -t snapreq .
docker run -d -p 127.0.0.1:7878:7878 -v snapreq-data:/data snapreq
scripts/docker-smoke.py http://127.0.0.1:7878     # optional check
```

or `docker compose up -d` with the included `docker-compose.yml`. The image is distroless and runs as
a non-root user; state and the workspace are in the `/data` volume. Inside the container the server
listens on all interfaces with `SNAPREQ_ALLOW_REMOTE=1`, so cloud metadata endpoints are blocked by
default; publish the port on `127.0.0.1` unless the network is trusted.

## Outbound request policy

The server sends requests for whoever calls its API, so it can be used to reach anything the machine
can reach (SSRF). The `[outbound]` table of `config.toml` limits that:

```toml
[outbound]
deny = ["10.0.0.0/8", "*.internal.example.com"]   # always blocked
allow = ["api.example.com", "*.staging.example.com", "192.168.50.0/24"]   # if set, nothing else
block_private = true      # loopback, 10/8, 172.16/12, 192.168/16, 100.64/10, fc00::/7, link-local, 0.0.0.0
block_metadata = true     # cloud metadata endpoints (169.254.169.254, fd00:ec2::254, metadata.google.internal …)
                          # and link-local; default: on when --allow-remote is set, off otherwise
```

Rules are CIDRs, single IPs, host names or `*.domain` (the domain and its subdomains). A `deny`
match always blocks; an `allow` match always permits (even a private address); then `block_metadata`
and `block_private` apply; finally, a non-empty `allow` list blocks everything it does not match.

The engine checks **every address it connects to, after DNS**, so names that resolve to internal
addresses, `--resolve` pins and redirects are covered. Addresses of a name that are blocked are
skipped (noted in the trace); if none is left the request fails in the connect phase with
`blocked by the outbound request policy: <address>: <reason>`. Through a proxy, the proxy address is
checked, and the target by name (or IP literal) when the proxy resolves it.

A server reachable from the network without any policy logs a warning at start.

## Conventions

- Base path `/api/v1`; JSON in and out.
- Errors: an HTTP status and `{ "error": { "kind": "bad_request" | "not_found" | "conflict" | "invalid" |
  "internal", "message": "…" } }`. `invalid` (422) is a file or request that does not validate.
- Collection ids, request and flow keys use letters, digits, `_`, `-` and `.` per segment and cannot
  leave the collection or name collection files (`snapreq`, `folder`, `environments/…`, `.…`).

## OpenAPI

`GET /api/v1/openapi.json` returns an OpenAPI 3.1 description of every endpoint below (except the
WebSocket, which OpenAPI cannot describe). It is generated from the route handlers (`utoipa`), so it
always matches the server. Request and response models of the core (requests, responses, collection
files) are described as free-form objects; their fields are in `docs/FORMAT.md` and `docs/CLI.md`.

## Health

| Method and path | Result |
|---|---|
| `GET /healthz` | `{ "status": "ok" }` |
| `GET /api/v1/version` | `{ "name": "snapreq", "version": "0.1.0", "api": 1 }` |

## Executing requests

`POST /api/v1/requests/execute` with either a complete request (the core JSON model, as in
`snapreq send --format json` / `docs/FORMAT.md`):

```json
{ "request": { "method": "GET", "url": "https://api.example.com/users", "headers": [{ "name": "Accept", "value": "application/json" }] } }
```

or a collection request:

```json
{ "collection": "shop", "key": "users/create", "env": "dev", "vars": { "name": "snap" } }
```

The response is `{ "history_id", "response", "error", "assertions"?, "captures"? }`: `response` is the
core response (status, headers, body, timings, redacted trace) or `null`, `error` is
`{ "kind": "execution", "message", "phase", "timed_out", "trace" }` when sending failed. Collection
requests also report their `[assert]` results and `[capture]` values (secret ones as `<redacted>`).
A request that cannot be built (undefined variable, bad URL) is a 422. Every execution is stored in
the history.

Two optional fields:

- `"file": { … }` (with `collection` and `key`): unsaved contents of the request file, sent instead of
  the saved one with the same inheritance, environment and variables (the web UI sends edits this
  way). `key` may also name a new request in an existing folder. Nothing is written.
- `"capture_wire": true`: record the raw bytes sent and received (a `wire` trace event), whatever the
  request's settings say. Credentials in the wire log are redacted like everywhere else.

## History

| Method and path | Result |
|---|---|
| `GET /api/v1/history?url=&method=&status=&class=&failed=&collection=&before=&limit=` | `{ "entries": [summary…] }`, newest first. `url` matches a substring (case-insensitive); `class` is a status class (`4` = 400–499); `failed=true` lists only requests without a response (`false`: only responses); `before` is an id for paging; `limit` 1–1000 (default 50). |
| `GET /api/v1/history/{id}` | The summary plus the stored `request` and `response` (redacted). |
| `DELETE /api/v1/history/{id}` | 204 |
| `DELETE /api/v1/history` | `{ "deleted": n }`: clears the history |

A summary is `{ "id", "created_at" (Unix ms), "method", "url", "status", "duration_ms", "error",
"collection", "request_key" }`.

## Runs

`POST /api/v1/runs` runs a flow, a folder or a whole collection to completion and stores the result:

```json
{ "collection": "shop", "target": "orders/checkout", "env": "staging",
  "vars": { "item": "sku-1" }, "data": [{ "user": "ann" }, { "user": "bob" }],
  "bail": false, "delay_ms": 0, "parallel": 1 }
```

`target` is a flow or folder key (absent: every request); `data` (one iteration per row) and
`iterations` exclude each other. The response is the run document of `snapreq run --format json`
(`docs/CLI.md`) plus `"id"` and `"collection"`.

| Method and path | Result |
|---|---|
| `GET /api/v1/runs?limit=` | `{ "runs": [{ "id", "created_at", "kind", "target_key", "name", "passed" }] }`, newest first |
| `GET /api/v1/runs/{id}` | The stored detailed report (with responses and traces) |

## Collections

| Method and path | Body | Result |
|---|---|---|
| `GET /api/v1/collections` | | `{ "collections": [{ "id", "name", "description", "requests", "flows" }] }` (or `{ "id", "error" }` for one that does not load) |
| `POST /api/v1/collections` | `{ "id", "name"? }` | 201 `{ "id", "file" }`; 409 if it exists |
| `GET /api/v1/collections/{id}` | | `{ "id", "file", "items": [tree], "flows": [{ "key", "name", "steps" }], "environments": [names] }` |
| `PUT /api/v1/collections/{id}` | `snapreq.toml` as JSON | the saved file |
| `DELETE /api/v1/collections/{id}` | | 204; removes the directory |
| `GET /api/v1/collections/{id}/requests/{key}` | | `{ "key", "file" }` (a request file as JSON) |
| `PUT /api/v1/collections/{id}/requests/{key}` | a request file as JSON | validated and written to `{key}.toml` (folders are created) |
| `DELETE /api/v1/collections/{id}/requests/{key}` | | 204 |
| `GET` / `PUT` / `DELETE /api/v1/collections/{id}/flows/{key}` | a flow file as JSON | `{key}.flow.toml` |
| `GET` / `PUT` / `DELETE /api/v1/collections/{id}/environments/{name}` | `{ "var": "value", "token": { "secret": true } }` | `environments/{name}.toml`; secret values are never returned or written |
| `GET` / `PUT` / `DELETE /api/v1/collections/{id}/folders/{key}` | `folder.toml` as JSON (`{}` for a plain folder) | `{ "key", "file" }`; `PUT` creates the directory; `DELETE` removes it with its contents |
| `POST /api/v1/collections/{id}/move` | `{ "kind": "request" \| "folder" \| "flow", "from", "to"?, "name"?, "place"?: { "before"? } }` | `{ "key" }`: moves into folder `to` (`""` = root), renames to `name`, and with `place` puts it before sibling `before` (or last) by writing the folder's `order`; 409 if the target exists |
| `PUT /api/v1/collections/{id}/secrets/{env or shared}/{name}` | `{ "value" }` | 204; like `snapreq secret set`: written to `.secrets/<env>.toml` (or `shared.toml`), never returned |
| `GET /api/v1/collections/{id}/variables?key=&env=` | | `{ "variables": [{ "name", "value", "secret", "has_value", "scope" }] }`: the variables the request at `key` sees with `env`, winning definition first-seen; `value` is `null` for secrets |

Tree items are `{ "type": "request", "key", "name", "method" }` or
`{ "type": "folder", "key", "name", "items": [...] }`. File JSON uses the same keys as the TOML files
(`docs/FORMAT.md`), e.g. `{ "name": "Create user", "method": "POST", "url": "{{base}}/users",
"headers": { "X-Team": "shop" }, "assert": { "status": 201 } }`.

## Live traces and run progress (WebSocket)

`GET /api/v1/ws` upgrades to a WebSocket. The client sends one JSON message per job; jobs on one
connection run one after another, and `id` (any JSON value, optional) is echoed on every message about
that job.

```json
{ "type": "execute", "id": 1, "request": { "method": "GET", "url": "https://api.example.com/users" } }
{ "type": "execute", "id": 2, "collection": "shop", "key": "users/create", "env": "dev" }
{ "type": "run", "id": 3, "collection": "shop", "target": "orders/checkout", "env": "staging" }
```

The bodies are the same as `POST /api/v1/requests/execute` and `POST /api/v1/runs`. While the job
runs, the server sends:

- `{ "type": "trace", "id", "entry": { "at_ms", "event", ... } }` for every trace event as it happens
  (redacted with the secrets known at that moment; the final result is authoritative). With
  `parallel` above 1, events of different iterations interleave.
- `{ "type": "progress", "id", "event": "request_started", "iteration", "key" }` and
  `{ "type": "progress", "id", "event": "request_done", "iteration", "key", "passed", "status",
  "duration_ms" }` for runs.

and finally `{ "type": "result", "id", "result": … }` (the same document as the REST endpoint, and
stored in history or runs the same way) or `{ "type": "error", "id", "error": { "kind", "message" } }`.
A malformed message gets an `error` and the connection stays open.

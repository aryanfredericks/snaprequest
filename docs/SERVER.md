# SnapRequest server

`snapreq serve` runs the self-hosted server: a JSON API (this document), and in Phase 7 the web UI.
Configuration (bind address, data directory, workspace, retention) is described in `docs/CLI.md`,
"Server".

- **Workspace:** the collections the API serves, one directory each (`<workspace>/<id>/snapreq.toml`).
  The directory name is the collection **id**. Files are the normal collection format, so they can be
  edited by hand, with the CLI, or kept in git.
- **Storage:** history and run results in `<data dir>/snapreq.db`, redacted (see `docs/CLI.md`).
- **Security:** no authentication yet (Phase 8). The server listens on loopback only unless
  `--allow-remote` is given (ADR-018).

## Conventions

- Base path `/api/v1`; JSON in and out.
- Errors: an HTTP status and `{ "error": { "kind": "bad_request" | "not_found" | "conflict" | "invalid" |
  "internal", "message": "…" } }`. `invalid` (422) is a file or request that does not validate.
- Collection ids, request and flow keys use letters, digits, `_`, `-` and `.` per segment and cannot
  leave the collection or name collection files (`snapreq`, `folder`, `environments/…`, `.…`).

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

## History

| Method and path | Result |
|---|---|
| `GET /api/v1/history?url=&method=&status=&before=&limit=` | `{ "entries": [summary…] }`, newest first. `url` matches a substring (case-insensitive); `before` is an id for paging; `limit` 1–1000 (default 50). |
| `GET /api/v1/history/{id}` | The summary plus the stored `request` and `response` (redacted). |

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

Tree items are `{ "type": "request", "key", "name", "method" }` or
`{ "type": "folder", "key", "name", "items": [...] }`. File JSON uses the same keys as the TOML files
(`docs/FORMAT.md`), e.g. `{ "name": "Create user", "method": "POST", "url": "{{base}}/users",
"headers": { "X-Team": "shop" }, "assert": { "status": 201 } }`.

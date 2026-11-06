# SnapRequest CLI

```
snapreq send [METHOD] URL [OPTIONS]
snapreq send REQUEST_FILE.toml [-e ENV] [--var name=value] [OPTIONS]
snapreq send -c COLLECTION_DIR REQUEST_KEY [-e ENV] [OPTIONS]
snapreq run [COLLECTION | FOLDER | FLOW] [-e ENV] [--var name=value] [--data FILE | -n N]
snapreq serve [--bind IP:PORT] [--data-dir DIR]
snapreq ls | new | secret | tokens …
```

`METHOD` defaults to `GET`, or `POST` when a body option is given. A URL without a scheme gets
`http://` (like curl). Run `snapreq send --help` for the full list of options.

## Examples

```sh
snapreq send https://httpbin.org/get
snapreq send https://api.example.com/search -q term=rust -q page=2
snapreq send POST https://api.example.com/users --json '{"name":"snap"}'
snapreq send PUT https://api.example.com/users/1 --json @user.json -H 'Authorization: Bearer TOKEN'
snapreq send https://api.example.com/upload -F title=Holiday -F photo=@beach.jpg
snapreq send https://internal.test/health --cacert company-ca.pem
snapreq send https://staging.example.com/ --resolve staging.example.com:443:10.0.0.5
```

## Collections

Requests saved in a [collection](FORMAT.md) run with the same `send` command:

```sh
snapreq send my-api/users/create-user.toml -e dev        # a request file
snapreq send -c my-api users/create-user -e prod         # a request key in a collection
cd my-api && snapreq send users/create-user              # from inside the collection
snapreq send my-api/health.toml --var base_url=http://localhost:9000
```

| Option | Meaning |
|---|---|
| `-c, --collection DIR` | The collection; TARGET is then a request key like `users/create-user`. |
| `-e, --env NAME` | Use `environments/NAME.toml`. |
| `--var name=value` | Set a variable for this run; it wins over every other scope (repeatable). |

Headers, settings and variables are inherited as described in [FORMAT.md](FORMAT.md). Options given
on the command line win over the files for that run: `-H` replaces headers of the same name, `-q`
adds query parameters, a body option replaces the body, and every connection, TLS and output option
overrides the matching setting. Options you do not give leave the files' settings alone.

Other collection commands:

```sh
snapreq ls [DIR]                                   # folders, requests, environments, flows; `[auth: …]` marks where auth is set
snapreq new collection my-api [--name "My API"]    # creates my-api/snapreq.toml
snapreq new request my-api/users/get.toml GET '{{base_url}}/users/{{id}}' --name "Get user"
snapreq secret set token -e prod [-c my-api]       # prompts for the value (hidden)
echo "$TOKEN" | snapreq secret set token -e prod   # or reads it from stdin
```

### Importing (`snapreq import`)

```sh
snapreq import shop.postman_collection.json --env dev.postman_environment.json   # Postman v2.1 + environments
snapreq import openapi.yaml --into petstore           # OpenAPI 3.0/3.1, JSON or YAML
snapreq import insomnia-export.json                   # Insomnia v4 JSON or v5 YAML
snapreq import recording.har                          # browser/proxy recording
snapreq import smoke.hurl                             # Hurl file → smoke/flow.flow.toml
snapreq import - --into scratch < copied-curl.txt      # one or more curl commands
```

The format is detected (`--format curl|postman|openapi|insomnia|har|hurl` overrides it). The collection is
written into `--into DIR` (default: a directory named after the collection), which must be missing or empty;
`--name` sets its name and `--json` prints a summary. Things that could not be imported exactly (scripts,
unsupported auth or asserts, file paths to copy, variables nothing defines) are printed as warnings.

Postman pre-request and test scripts become `[script]` tables: `pm.test`, `pm.expect` (Chai chains are
supported as they are), `pm.response.*`, `pm.environment/collectionVariables/variables/globals.get/set` and the
legacy `tests[...]`/`postman.*` API are rewritten to the `snapreq` API (see FORMAT.md, "Scripts"). Lines
using something without an equivalent (`pm.sendRequest`, `pm.cookies`, `require`, …) are commented out, or the
whole script is kept as a comment, with a warning.

Credentials in the file (passwords, tokens, API keys, cookies, client secrets) are never written into
collection files: they become secret variables, and their values are stored in `.secrets/` (sealed when
`SNAPREQ_SECRET_KEY` is set). What each format maps to is described in ADR-029 (`docs/DECISIONS.md`).

Secret values are looked up in `--var`, then `SNAPREQ_SECRET_<NAME>` environment variables, then
`.secrets/<env>.toml` and `.secrets/shared.toml` (never committed; see FORMAT.md). They are hidden in
every trace and output, including when given with `--var`.

## Server (`snapreq serve`)

```sh
snapreq serve                                        # http://127.0.0.1:7878
snapreq serve --bind 127.0.0.1:9000 --data-dir ./snapreq-data
snapreq serve --bind 0.0.0.0:7878                    # accounts on: create users with `snapreq user add`
```

Starts the self-hosted server (API and web UI, docs/SERVER.md). It prints
`snapreq server listening on http://ADDR (data: DIR)` on stderr, logs one line per API request (`info`
level; change with `--log-level` or `SNAPREQ_LOG`) and stops gracefully on Ctrl-C or SIGTERM.

| Setting | Flag | Environment | Config file key | Default |
|---|---|---|---|---|
| Listen address | `--bind IP:PORT` | `SNAPREQ_BIND` | `bind` | `127.0.0.1:7878` |
| Data directory | `--data-dir DIR` | `SNAPREQ_DATA_DIR` | `data_dir` | `$XDG_DATA_HOME/snapreq`, `~/.local/share/snapreq`, `~/Library/Application Support/snapreq`, `%APPDATA%\snapreq` |
| Config file | `--config FILE` | `SNAPREQ_CONFIG` | | `config.toml` in the data directory, if present |
| Login | `--auth none\|accounts` | `SNAPREQ_AUTH` | `[auth] mode` | `none` on loopback, `accounts` otherwise |
| Non-loopback address without accounts | `--allow-remote` | `SNAPREQ_ALLOW_REMOTE=1` | `allow_remote = true` | off |

Flags win over environment variables, which win over the config file. Unknown config keys are errors.

The server keeps request history and run results in `snapreq.db` (SQLite) in the data directory.
Nothing secret is stored: credentials, sensitive headers and known secret values are replaced with
`<redacted>` (in URLs, headers, bodies and traces), auth is stored as its type only, and response
bodies are cut at 1 MiB. Old entries are pruned at start and every hour:

```toml
# config.toml
[history]
max_age_days = 30     # default 30; 0 = keep forever
max_mb = 512          # history and runs together; default 512; 0 = no limit
```

The server sends requests on behalf of whoever calls it. On a non-loopback address it therefore turns
accounts on (a login for the UI, a token for scripts; docs/SERVER.md, "Accounts"); serving such an
address without accounts needs `--allow-remote`, which says the network is trusted.

### Accounts (`snapreq user`)

```sh
snapreq user add ana                                 # asks for the password twice (hidden)
echo "$PASSWORD" | snapreq user add ci-bot           # or the first line of stdin
snapreq user list
snapreq user passwd ana                              # also logs ana out everywhere
snapreq user remove ana                              # with her tokens, history, runs and memberships
```

`snapreq audit [-n 50]` prints the server's audit log (docs/SERVER.md, "Hardening").
`snapreq backup FILE|- [--with-key]` and `snapreq restore FILE|- [--force]` back up and restore the
server's data (docs/SERVER.md, "Backup and restore").

They work on the server's database (`--data-dir`, `--config` and the environment variables as for
`snapreq serve`), also while the server runs. User names are case-insensitive; passwords need at least
8 characters. The first account owns the `default` workspace; other users are added to workspaces in the
UI or the API (docs/SERVER.md, "Workspaces"). A user who is the only owner of a shared workspace cannot
be removed until someone else is made an owner.

Endpoints so far: `GET /healthz` → `{"status":"ok"}`, `GET /api/v1/version` →
`{"name":"snapreq","version":"…","api":1}`.

## Runs (`snapreq run`)

```sh
snapreq run my-api                                   # every request of the collection, in tree order
snapreq run my-api/users                             # a folder (or: -c my-api users, --folder users)
snapreq run my-api/orders/checkout.flow.toml -e staging   # a flow (or: -c my-api orders/checkout)
snapreq run -c my-api --folder users --data users.csv     # once per data row
snapreq run -c my-api orders/checkout -n 5                # five times
```

Without a TARGET, `run` uses the collection around the current directory. With `-c DIR`, TARGET is a flow
key or a folder key.

- **Collections and folders:** every request is sent in order through one run, so values captured by one
  request are available to the next; a failing request does not stop the run.
- **Flows** (see `docs/FORMAT.md`, "Flows") stop at the first failing step.
- **Iterations:** `-n N` runs everything N times; `--data FILE` runs once per row of a CSV file (first row:
  variable names; RFC 4180 quoting) or a JSON file (array of objects). Every iteration starts without
  captured values. Data values rank below `--var` and step variables and above everything else; values of
  variables declared secret are hidden. `-n` and `--data` cannot be combined.
- **Run control:** `--bail` stops at the first failing request (the rest of the iteration and later
  iterations are skipped and counted as such); `--delay MS` pauses between requests and flow steps;
  `--parallel N` runs up to N iterations at the same time. Requests inside one iteration always run in
  order, because later ones may use values captured by earlier ones; results are reported in order.

One line per request or step, with failing checks and missing captures underneath; `-v` lists every check
and capture, `-vv` adds the trace (with wire bytes) of failures.

```text
Checkout  orders/checkout
✓  1  auth/login    POST https://api.example.com/login → 200  41.2 ms
✓  2  Create order  POST https://api.example.com/orders → 201  63.0 ms
✗  3  orders/get    GET https://api.example.com/orders/17 → 200  12.4 ms  20 attempts
      ✗ json $.state equals "paid"  (actual: "pending")
flow failed: 2 of 4 steps passed, 1 skipped
```

Options: `-e ENV`, `--var name=value` (wins over everything), `--folder`, `-n N`, `--data FILE`, `--bail`,
`--delay MS`, `--parallel N`, `--report FORMAT:FILE`, `-v`/`-vv`,
`--format pretty|json`, `--color`, `-k` (no TLS verification), `-t SECONDS` (per request), `--secret VALUE`.

### Reports

`--report FORMAT:FILE` (repeatable) writes a report after the run; parent directories are created.

| Format | Contents |
|---|---|
| `junit` | JUnit XML for CI: one `<testsuite>` per iteration, one `<testcase>` per request or step. Failed checks and captures are `<failure>`s (expected vs actual), requests that could not be built or sent are `<error>`s, requests not run are `<skipped/>`. |
| `html` | One self-contained page (no scripts or external files; light and dark themes): per request the checks, captures, timing bars, response headers and body (up to 64 KiB) and the trace. Failures are expanded. |
| `json` | The `--format json` document plus each response's version, headers, body, timings and full trace. |
| `har` | Every exchange as an HTTP Archive (HAR 1.2), for browser dev tools and HAR viewers. Credential headers (`Authorization`, `Cookie`, `Set-Cookie`, API keys) are always `<redacted>`. |

Reports hide every known secret (credentials, secret variables, secret captures, `--secret` values) in
traces, URLs, checks **and response bodies**, since they are meant to be shared. Values of data columns
named like declared secret variables are shown as `<redacted>`.

```sh
snapreq run my-api -e ci --report junit:reports/junit.xml --report html:reports/run.html
```

Exit codes: 0 everything passed; otherwise the worst problem: 3 a request could not be built, 5 a network
failure, 6 a timeout, 7 a check or capture failed.

`--format json`:

```json
{ "format": 1,
  "target": { "kind": "flow" | "folder" | "collection", "key": "orders/checkout", "name": "Checkout" },
  "passed": false,
  "planned_iterations": 1,
  "iterations": [{ "number": 1, "data": { "user": "alice" } | null, "passed": false, "total": 4, "skipped": 1,
    "steps": [{ "number": 1, "title": "auth/login", "key": "auth/login", "passed": true, "attempts": 1,
      "request": { "method": "POST", "url": "…" }, "response": { "status": 200, "duration_ms": 41.2 },
      "error": null, "assertions": […], "until": […], "captures": […] }] }] }
```

## Assertions

A request file's `[assert]` checks (see `docs/FORMAT.md`) run after the response arrives. Results go to
stderr, one line per check, then a summary; the exit code is 7 if any check failed. `--no-assert`
skips them. Credential and secret values are hidden in the reported actual values.

With `-v`, the request file's `[capture]` values are listed (`captured user_id = 7`; secret captures as
`<redacted>`). A single `send` does not keep them; `snapreq run` and flows pass them to later requests.

```text
✓ status equals 201
✗ json $.id type integer  (actual: "abc")
assertions: 1 passed, 1 failed
```

Scripts (`[script]` in collection, folder and request files, see `docs/FORMAT.md`) run with `send` and
`run`: pre-request scripts before the request is built, test scripts after the response. Their
`snapreq.test(...)` results are listed with the checks (`✓ test token is set`) and count for the exit code;
`console.log` lines go to stderr as `log: …` (in `snapreq run`, with `-v` or when the request failed).

## Output

JSON responses (`application/json`, `*+json`, or JSON-looking text) are pretty-printed with
colors, keeping the server's key order. Binary bodies are summarized on a terminal instead of
being dumped into it; when stdout is redirected they are written byte for byte.

## Verbose trace (`-v`, `-vv`)

The trace is written to stderr in a curl-like style: `*` for what SnapRequest did, `>` for what was
sent, `<` for what came back, `!` for the failure. Credentials are always redacted.

- `-v`: DNS result, connection, TLS version, cipher, ALPN and server certificate, request and response
  headers, the duration of every phase, redirects and cookie notes, and the total time.
- `-vv`: everything, with a millisecond timestamp on each line, the full certificate chain (issuer,
  validity, names, serial, SHA-256 fingerprint) and the raw bytes of each HTTP/1.1 exchange.

`-v` ends with a timing waterfall: each phase drawn as a bar on one timeline, one block per
redirect hop (also available on stdout with `--print t`):

```
                     0                                                  662.1 ms
hop 1/2: 301 → https://github.com/
DNS         169.5 ms ███████████████
connect     199.6 ms                ██████████████████
send         0.13 ms                                  █
wait        204.3 ms                                  ██████████████████
download     0.13 ms                                                    █
hop 2/2: final response
DNS          0.49 ms                                                    █
connect      13.4 ms                                                    █
TLS          19.2 ms                                                     ██
…
```

When a request fails, `-v` shows the trace up to the failure. Without `-v` the error message
suggests rerunning with it.

```
$ snapreq send https://example.com/ -v -p ''
* resolved example.com → 2606:4700:…, 104.20.23.154 (11.6 ms)
* connected to [2606:4700:…]:443 from […]:41570 (7.90 ms)
* TLSv1.3 TLS13_AES_256_GCM_SHA384, ALPN h2 (11.7 ms)
*   server certificate: CN=example.com
*     valid until 2026-12-25T22:56:35Z
> GET https://example.com/ HTTP/2
> user-agent: snapreq/0.1.0
…
< HTTP/2 200 OK
< content-encoding: br
…
* 334 B body received (0.85 ms)
* decoded br body: 334 → 577 bytes
* total 50.6 ms
```

## Request options

| Option | Meaning |
|---|---|
| `-H, --header 'Name: value'` | Add a header (repeatable). |
| `-q, --query name=value` | Add a query parameter (repeatable). |
| `-d, --data BODY` | Raw body text; `@path` sends a file. No content type is implied (text/plain for text). |
| `--json JSON` | JSON body; `@path` sends a JSON file. Sets `Content-Type: application/json`. |
| `--form name=value` | URL-encoded form field (repeatable). |
| `-F, --multipart name=value` | Multipart field; `name=@path[;type=mime]` uploads a file (repeatable). |
| `--file PATH` | Send a file as the raw body (content type guessed from the extension). |
| `--graphql QUERY` | GraphQL query (`@file.graphql` reads a file), sent as JSON with POST unless a method is given; with `--variables JSON` (`@file` too) and `--operation NAME`. |

Only one kind of body can be given per request.

`snapreq introspect URL|REQUEST` fetches a GraphQL schema by introspection, with the request's headers,
auth and settings, and prints it as SDL (`--format json`: the `__schema` object):

```sh
snapreq introspect https://api.example.com/graphql -H 'Authorization: Bearer TOKEN' > schema.graphql
snapreq send https://api.example.com/graphql --graphql 'query { me { id } }'
```

## Auth options

| Option | Meaning |
|---|---|
| `-u, --user USER[:PASSWORD]` | Basic auth. Without `:PASSWORD` you are asked for it on a terminal (keeps it out of shell history); when stdin is not a terminal the password is empty. |
| `--digest` | Use Digest auth (RFC 7616) with the `-u` credentials: the request is sent without credentials, and once more with the answer if the server replies `401` with a Digest challenge. MD5, SHA-256 and SHA-512-256 (and `-sess`), `qop=auth`. |
| `--bearer TOKEN` | `Authorization: Bearer TOKEN`. |
| `--api-key NAME=VALUE` | API key in the header `NAME`, or in the query parameter `NAME` with `--api-key-in query`. |
| `--api-key-in header\|query` | Where `--api-key` goes (default `header`). |
| `--oauth2 GRANT` | OAuth 2.0: `client-credentials`, `password` (user credentials from `-u`) or `authorization-code` (PKCE, signs in through the browser). Needs `--token-url` and `--client-id`. |
| `--token-url URL`, `--auth-url URL` | Token endpoint; authorization endpoint (authorization code grant). |
| `--client-id ID`, `--client-secret SECRET` | Client credentials; omit the secret for public clients. Sent with HTTP Basic. |
| `--scope SCOPES` | Space-separated scopes. |
| `--redirect-uri URI` | Loopback redirect URI for the authorization code grant (default `http://127.0.0.1:<random port>/callback`). Register it with the provider. |
| `--aws-sigv4 REGION/SERVICE` | AWS Signature V4, for example `eu-west-1/s3` or `us-east-1/execute-api`. Credentials from `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` and optionally `AWS_SESSION_TOKEN`. Every hop is signed; the query string is sent in strict RFC 3986 encoding. Set `-H 'x-amz-content-sha256: UNSIGNED-PAYLOAD'` to skip hashing large S3 uploads. |
| `--no-auth` | Send no auth, even if the collection defines one. |

Only one auth option can be given. With a collection request, an auth option replaces the
collection's `[auth]`. Auth is applied to every hop of a redirect chain on the same origin and
dropped when a redirect goes to another origin. A Digest challenge splits the trace and the waterfall
like a redirect: phase timings describe the final, authenticated exchange. A header given with `-H` (or in the request file)
with the same name wins over the auth option; the trace says so. Credential values are hidden in
all output, including a query API key in URLs.

### OAuth 2.0 tokens

Access tokens are cached in `oauth-tokens.json` in the user cache directory (`$SNAPREQ_CACHE_DIR`,
else `$XDG_CACHE_HOME/snapreq` or `~/.cache/snapreq` on Linux, `~/Library/Caches/snapreq` on macOS,
`%LOCALAPPDATA%\snapreq` on Windows). The file is readable by its owner only on Unix and never holds
client secrets or passwords. A token is reused until 10 s before it expires, then refreshed with its
refresh token (or requested again). If the API rejects a cached token with `401`, a new one is
fetched and the request sent once more (an `auth_challenge` event in the trace). Token requests use
the request's proxy, TLS and timeout settings; problems with them fail in the phase "getting an
access token" (exit code 5).

For the authorization code grant SnapRequest prints the sign-in URL, opens the browser (unless
`SNAPREQ_NO_BROWSER` is set) and waits up to 5 minutes for the redirect on the loopback address.

```sh
snapreq tokens path     # where the cache file is
snapreq tokens clear    # delete every cached token
```

## Connection options

| Option | Meaning |
|---|---|
| `-t, --timeout SECONDS` | Total time limit (default 30, `0` = none). |
| `--connect-timeout SECONDS` | DNS + TCP + TLS time limit (default 10, `0` = none). |
| `--http1.1` / `--http2` | Force HTTP/1.1, or require HTTP/2 (prior knowledge for plain HTTP). |
| `-x, --proxy PROXY` | `http://`, `socks5://`, `socks5h://` proxy URL, `direct`, or `env` (default: `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, `NO_PROXY`). Loopback hosts never use environment proxies. |
| `--resolve host:port:addr[,addr]` | Pin a host to addresses, skipping DNS (port may be `*`). |
| `--no-follow`, `--max-redirs N` | Redirect handling (default: follow up to 10). |
| `--no-decompress` | Do not request or decode compressed responses. |
| `--no-cookies` | Do not send or store cookies. |

## TLS options

| Option | Meaning |
|---|---|
| `-k, --insecure` | Skip certificate verification. The trace still reports what verification would have said. |
| `--cacert PATH` | Extra CA certificates (PEM) to trust. |
| `--cert PATH --key PATH` | Client certificate and key (PEM) for mutual TLS. |
| `--tls-min 1.2\|1.3`, `--tls-max 1.2\|1.3` | Allowed TLS versions. |

## Server-Sent Events (`snapreq sse`)

`snapreq sse` reads a `text/event-stream` response and prints each event as it arrives, in the stream's
own format (`id:`, `event:` and `data:` lines, a blank line between events), until the server ends the
stream, a limit is reached or Ctrl-C. It takes the options of `send` (collection requests, `-e`, auth,
TLS, `-v` for the trace) and sends `Accept: text/event-stream` unless the request sets `Accept`. A
request file's `[assert]` checks run on the response when the stream ends.

```sh
snapreq sse https://api.example.com/events --max-events 10
snapreq sse -c my-api prices/stream -e dev --duration 30 --event price
snapreq sse http://localhost:8080/feed --last-event-id 42 --format json
```

| Option | Meaning |
|---|---|
| `-n`, `--max-events N` | Stop after N events. |
| `--duration SECONDS` | Stop after this long. |
| `--event TYPE` | Only print (and count) events of this type (repeatable). |
| `--last-event-id ID` | Send `Last-Event-ID` to resume a stream. |

`--format json` prints one JSON object per event (`event`, `data`, `id`, `retry`, `at_ms`) and a final
`{"end": "server" | "max_events" | "duration" | "interrupted", "status", "events"}`. A response that is not
an event stream is printed with an error (exit code 4 for a status of 400 or more, else 5).

## WebSockets (`snapreq ws`)

`snapreq ws` opens a WebSocket, sends messages and prints what arrives: sent messages as `> …`, received
ones as `< …` (binary as hex), status lines on stderr. It takes the same request, auth, connection, TLS and
collection options as `send`; the handshake trace is shown with `-v`.

```sh
snapreq ws wss://echo.example.com/ws --send hello --count 1
snapreq ws ws://localhost:8080/chat -H 'Authorization: Bearer TOKEN' -i   # type lines to send
snapreq ws -c my-api chat/room -e dev --message join --wait 10
snapreq ws ws://localhost:8080/feed --format json > messages.ndjson
```

| Option | Meaning |
|---|---|
| `-m`, `--send TEXT` | Send a text message once connected (repeatable). |
| `--message NAME` | Send a saved message of the request file (`[[websocket.messages]]`); `all` sends every one. |
| `--subprotocol NAME` | Offer a subprotocol (repeatable; replaces the file's). |
| `-i`, `--interactive` | Also send each line typed on stdin (the default when nothing else is sent). |
| `-n`, `--count N` | Close after receiving N text or binary messages. |
| `--wait SECONDS` | Once everything is sent, close after this long without a message (default 2; `0` waits for the server). |

`--format json` prints one JSON object per line: `{"event": "open", …}`, messages as `{"kind", "data",
"direction", "at_ms"}` and `{"event": "close", …}`. Exit codes are those of `send`: 0 when the connection
ends normally, 5 when it cannot be opened (or fails), 6 on a timeout.

## Requests as code (`--export`)

`snapreq send … --export FORMAT` prints the request as code instead of sending it: `curl`, `python`
(requests), `javascript` (fetch, Node 18+), `go` (net/http) or `rust` (reqwest). It works for URLs and for
collection requests, with environments and `--var`:

```sh
snapreq send -c my-api users/create-user -e prod --export curl
snapreq send POST https://api.example.com/users --json '{"name":"snap"}' --export python
```

Variables are filled in, but secret values are never written: the code reads them from environment
variables named like the CLI's own (`${SNAPREQ_SECRET_TOKEN}` for the secret variable `token`;
credentials written into a file directly become `SNAPREQ_SECRET_1`, …). OAuth 2.0 requests read the access
token from `SNAPREQ_ACCESS_TOKEN`; AWS Signature V4 is kept in cURL (`--aws-sigv4`) and noted in the other
languages.

## Output options

| Option | Meaning |
|---|---|
| `-p, --print PARTS` | What to print: `H` request headers (as sent, credentials redacted), `B` request body, `h` status line and response headers, `b` response body, `m` timings and sizes, `t` timing waterfall. Default: `hb` on a terminal, `b` when piped, so `snapreq send URL \| jq` works. |
| `--format pretty\|json` | `json` prints one machine-readable document on stdout (see below). |
| `-o, --output FILE` | Save the decoded response body to FILE (overwritten) instead of printing it. The directory must exist; this is checked before the request is sent. |
| `--color auto\|always\|never` | Colors (default `auto`: only on a terminal, and never when `NO_COLOR` is set). |
| `-v` / `-vv` | Show the trace on **stderr** (stdout keeps only the `--print` parts). See below. |
| `-f, --fail` | Exit with code 4 when the status is 400 or higher. |
| `--secret VALUE` | Hide this value everywhere in the output (repeatable). Credentials in `Authorization`, `Cookie` and similar headers are hidden automatically. |

## JSON output (`--format json`)

One JSON document is written to stdout, for success and failure alike. Exit codes stay the same.

```json
{
  "format": 1,
  "request":  { "method": "GET", "url": "https://api.example.com/items?key=<redacted>" },
  "response": {
    "status": 200, "version": "HTTP/2", "url": "…",
    "headers": [{ "name": "content-type", "value": "application/json" }],
    "body": { "encoding": "utf8", "data": "{…}" },
    "remote_addr": "93.184.216.34:443",
    "timings": { "dns_ms": 11.6, "connect_ms": 7.9, "tls_ms": 11.7, "send_ms": 0.04,
                 "wait_ms": 17.8, "download_ms": 0.85, "total_ms": 50.6 },
    "sizes": { "request_headers": 77, "request_body": 0, "response_headers": 359,
               "response_body": 334, "response_body_decoded": 577 },
    "trace": [{ "at_ms": 0.01, "event": "dns_start", "host": "api.example.com" }, …]
  },
  "error": null
}
```

- `body.encoding` is `utf8` or `base64` (binary bodies).
- With `-o FILE`, `body_file` holds the path and `response.body` is empty.
- When the request file has `[assert]` checks, `assertions` lists them:
  `[{ "subject": "json $.id", "check": "type integer", "passed": false, "actual": "\"abc\"" }]`
  (`message` explains checks that could not be applied, and schema errors).
- When it has `[capture]` entries, `captures` lists them:
  `[{ "name": "user_id", "value": "7", "secret": false }]` (secret values `"<redacted>"`, `error` when missing).
- On failure `response` is `null` and `error` is
  `{ "kind": "execution", "phase": "connect", "timed_out": false, "message": "…", "trace": […] }`, or
  `{ "kind": "invalid_request", "message": "…" }` when nothing was sent.
- Durations are milliseconds. The trace is redacted (credentials and `--secret` values); `--secret`
  values are also removed from `request.url` and `response.url`. The body is returned exactly as received.
- `format` changes only on incompatible changes; new fields may be added at any time.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | A response was received (any status, unless `--fail`). |
| 1 | Unexpected internal error. |
| 2 | Invalid command line (unknown option, missing URL). |
| 3 | Invalid request: bad URL or option value, unreadable file. Nothing was sent. |
| 4 | `--fail` was given and the status was 400 or higher. |
| 5 | Network failure: DNS, connection, TLS or protocol error, or no OAuth 2.0 token could be obtained. |
| 6 | A connect or total timeout fired. |
| 7 | A response was received but an `[assert]` check of the request file failed. |

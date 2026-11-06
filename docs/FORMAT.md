# Collection format, version 1

A **collection** is a directory of plain TOML files, designed to be kept in git: one file per request,
small diffs, and no generated IDs.

```
my-api/
├── snapreq.toml              # the collection itself (required)
├── health.toml               # a request
├── users/                    # a folder
│   ├── folder.toml           # folder settings and order (optional)
│   ├── list-users.toml
│   ├── create-user.toml
│   ├── create-user.json      # a body file used by create-user.toml
│   └── onboarding.flow.toml  # a flow (see "Flows")
└── environments/             # environments (see "Environments")
    ├── dev.toml
    └── prod.toml
```

## Rules

- Every `*.toml` file in the collection directory or a folder is a request, except `snapreq.toml`,
  `folder.toml` and flows (`*.flow.toml`). Other files (body files, READMEs) are ignored.
- Every subdirectory is a folder, except `environments/` at the top level. Folders can be nested.
- Names starting with `.` are ignored (`.git`, editor files).
- An item's **key** is its file name without `.toml`, or the folder's directory name. Keys are used
  in `order` lists and on the command line (`users/create-user`).
- Unknown keys are errors, reported with the file and line, so typos are not silently ignored.
- Paths inside files (`file = ...`) are relative to the file that contains them.
- `{{name}}` placeholders may appear in any string. They are resolved when a request runs.

## `snapreq.toml`

```toml
format = 1                         # required; the format version
name = "My API"                    # required
description = "Optional text"
order = ["health", "users"]        # optional; see "Ordering"

[vars]                             # optional; default variable values (see "Variables")
base_url = "http://localhost:3000"

[headers]                          # optional; sent with every request in the collection
Accept = "application/json"

[auth]                             # optional; auth for every request (see "Auth")
type = "bearer"
token = "{{token}}"

[settings]                         # optional; defaults for every request (see "Settings")
timeout_ms = 10000
```

## Request files

```toml
name = "Create user"               # optional; defaults to the key
description = "Creates a user."    # optional
method = "POST"                    # optional; defaults to GET
url = "{{base_url}}/users"         # required

[vars]                             # optional; variables only this request sees
role = "admin"

[query]
notify = "true"

[headers]
Authorization = "Bearer {{token}}"

[body]
type = "json"
content = '''
{ "name": "snap" }
'''

[settings]
follow_redirects = false
```

### Key/value tables (`[query]`, `[headers]`, `[body.fields]`)

Entries keep the order in which they are written. Each value is one of:

| Form | Meaning |
|---|---|
| `Name = "value"` | One entry. |
| `Name = ["a", "b"]` | The same name sent several times, in this order. |
| `Name = { value = "x", enabled = false }` | Kept in the file but not sent. |
| `Name = ["a", { value = "b", enabled = false }]` | Mixed. |

Header names are case-insensitive when sent; they are written as you type them.

### Bodies

`[body]` has a `type`:

| `type` | Keys | Sent as |
|---|---|---|
| `"text"` | `content` or `file`, optional `content_type` | `text/plain; charset=utf-8` unless `content_type` is set |
| `"json"` | `content` or `file` | `application/json` |
| `"form"` | `[body.fields]` key/value table | `application/x-www-form-urlencoded` |
| `"multipart"` | `[[body.parts]]` | `multipart/form-data` |
| `"file"` | `file`, optional `content_type` | raw file; type guessed from the extension |
| `"graphql"` | `query` or `query_file`, optional `variables`, `operation_name` | `application/json`: `{"query", "variables", "operationName"}` |

`content` and `file` cannot both be set. A GraphQL request is sent with POST unless `method` says
otherwise; `variables` is a JSON object written as text, and placeholders work in it and in the query:

```toml
url = "{{base_url}}/graphql"

[body]
type = "graphql"
query_file = "queries/user.graphql"    # or query = '''query User($id: ID!) { user(id: $id) { name } }'''
variables = '{"id": "{{user_id}}"}'
operation_name = "User"                # optional; for documents with several operations
```

Multipart parts: Multipart parts:

```toml
[[body.parts]]
name = "title"
value = "Holiday"

[[body.parts]]
name = "photo"
file = "beach.jpg"                 # relative to this request file
filename = "beach.jpg"             # optional; defaults to the file name
content_type = "image/jpeg"        # optional; guessed from the extension
enabled = true                     # optional
```

### Auth

`[auth]` has a `type`. Every value may contain placeholders.

| `type` | Keys | Sent as |
|---|---|---|
| `"inherit"` | | The auth of the enclosing folder or collection. Same as no `[auth]` table. |
| `"none"` | | No auth, even if a folder or the collection has one. |
| `"basic"` | `username`, optional `password` | `Authorization: Basic base64(username:password)` (UTF-8) |
| `"bearer"` | `token` | `Authorization: Bearer <token>` |
| `"digest"` | `username`, optional `password` | Digest (RFC 7616): sent without credentials first, then again answering the server's `401` challenge (MD5, SHA-256, SHA-512-256, `-sess`, `qop=auth`) |
| `"oauth2"` | see below | `Authorization: Bearer <access token>`, fetched, cached and refreshed |
| `"aws_sigv4"` | `access_key`, `secret_key`, optional `session_token`, `region`, `service` | AWS Signature V4 on every hop (`X-Amz-Date`, `Authorization`; `X-Amz-Content-SHA256` for `service = "s3"`) |
| `"api_key"` | `name`, `value`, optional `location` | header `name: value`, or with `location = "query"` the query parameter `name=value` |

```toml
[auth]
type = "api_key"
name = "X-API-Key"
value = "{{api_key}}"
location = "header"                # "header" (default) | "query"
```

OAuth 2.0:

```toml
[auth]
type = "oauth2"
grant = "authorization_code"       # "client_credentials" | "password" | "authorization_code"
token_url = "https://login.example.com/oauth/token"
auth_url = "https://login.example.com/authorize"   # authorization_code only
client_id = "{{client_id}}"
client_secret = "{{client_secret}}"  # optional; omit for public clients
scope = "openid profile"             # optional, space-separated
username = "{{user}}"                # password grant only
password = "{{password}}"
redirect_uri = "http://127.0.0.1:8765/callback"  # optional; loopback http only
client_auth = "basic"                # "basic" (default) | "body": how the client authenticates

[auth.params]                        # optional extra parameters, e.g. for Auth0
audience = "https://api.example.com"
```

The authorization code grant always uses PKCE (S256) and a random `state`. Tokens are shared by every
request with the same grant, token URL, client id, scope and user; how they are cached depends on the
front end (the CLI keeps them in the user cache directory, see `docs/CLI.md`).

Auth is applied to every hop of a redirect chain on the same origin and dropped on a redirect to
another origin. A header with the same name set in `[headers]` wins over `[auth]`. Credential values
are hidden in traces and output, whether or not they come from secret variables; keeping them in
secret variables keeps them out of the collection files.

### Settings

All keys are optional; anything not set comes from the folder, then the collection, then the
built-in defaults.

```toml
[settings]
timeout_ms = 30000
connect_timeout_ms = 10000
follow_redirects = true
max_redirects = 10
decompress = true
cookies = true
http_version = "auto"              # "auto" | "http1" | "http2"
proxy = "env"                      # "env" | "direct" | proxy URL
verify_tls = true
ca_cert = "certs/company-ca.pem"   # relative to this file
client_cert = "certs/me.crt"
client_key = "certs/me.key"
tls_min_version = "1.2"            # "1.2" | "1.3"
tls_max_version = "1.3"
```

### Assertions

`[assert]` lists checks run on the response. Every check that is set must pass.

```toml
[assert]
status = 201                       # or "2xx", or [200, 201]
duration_ms = { lt = 800 }         # total time of the exchange
body = { contains = "snap" }       # the body as text
schema = "schemas/user.json"       # JSON Schema the body must match; relative to this file

[assert.headers]                   # header name (case-insensitive) → check
content-type = { starts_with = "application/json" }
x-request-id = { exists = true }

[assert.json]                      # JSONPath (RFC 9535) → check
"$.id" = { type = "integer", gt = 0 }
"$.name" = "{{name}}"              # a plain value means `equals`
"$.tags" = { length = 2, contains = "admin" }
"$.user" = { equals = { role = "owner" } }   # compare with a table through `equals`
```

A check is a plain value (shorthand for `equals`) or a table of operators:

| Operator | Passes when the actual value … |
|---|---|
| `equals`, `not_equals` | is (not) equal. Numbers compare by value (`7` equals `7.0`); header and body text match unquoted numbers and booleans (`x-count = 3`). |
| `one_of = [..]` | equals one of the values. |
| `contains`, `not_contains` | (does not) contain a substring (strings), an element (arrays) or a key (objects). |
| `starts_with`, `ends_with` | starts or ends with the text. |
| `matches` | matches the regular expression somewhere (Rust `regex` syntax; use `'^...$'` to anchor). |
| `exists` | is present (`true`) or absent (`false`). Every other operator fails on a missing value. |
| `type` | is a `string`, `number`, `integer`, `boolean`, `array`, `object` or `null`. |
| `length` | has this many characters, elements or keys. |
| `lt`, `lte`, `gt`, `gte` | is a number (or numeric text) below / above the bound. |

- `status` also accepts a class such as `"2xx"` and a list meaning "one of".
- A JSONPath that matches one value is checked against that value; one that matches several is
  checked against the array of them; one that matches nothing is missing.
- Placeholders work in expected values, patterns and the schema path.
- Invalid JSONPaths, regular expressions and schema files are reported before the request is sent.
- Assertions belong to the request file; they are not inherited from folders or the collection.

### Captures

`[capture]` takes values from the response into variables. Within one run (`snapreq run`, a flow) later
requests see them as runtime variables, the highest-precedence scope.

```toml
[capture]
user_id = "$.user.id"                               # a plain string is a JSONPath into the body
token = { json = "$.access_token", secret = true }  # hidden in every later trace and report
next = { header = "Location" }                      # first value, name case-insensitive
session = { cookie = "sid" }                        # from Set-Cookie
csrf = { regex = 'name="csrf" value="([^"]+)"' }    # group 1 if the pattern has groups, else the match
code = { status = true }
raw = { body = true }
```

- Each capture has exactly one source; `group = N` picks a regex group.
- JSON strings are captured as their text; other JSON values (numbers, arrays, objects) as compact JSON.
  A JSONPath matching several values captures them as an array.
- A source that is missing fails the capture (the run reports it); the variable stays undefined, so a
  later request that uses it fails with "variable is not defined".

### Scripts

`[script]` holds JavaScript run before the request is built (`pre_request`) and after its response
(`test`), inline or from a file next to the request file:

```toml
[script]
pre_request = '''
snapreq.variables.set("ts", Date.now());
snapreq.request.headers.set("X-Signature", "{{sig}}");
'''
test_file = "checks/create-user.js"     # or `test = '''…'''`; not both
```

`snapreq.toml`, `folder.toml` and flow steps (`[step.script]`) may have one too. For a request they run in
this order: the collection's, each enclosing folder's (outermost first), the request's, then the flow
step's. A failing pre-request script stops the request; what earlier scripts did still counts.

The `snapreq` object:

| API | Meaning |
|---|---|
| `snapreq.variables.get(name)`, `.has(name)`, `.toObject()` | Variables the request sees (secret values included). |
| `snapreq.variables.set(name, value)` | A runtime variable for the rest of the run, like a captured value. Values are turned into strings (objects as JSON). |
| `snapreq.variables.setSecret(name, value)` | The same, hidden in every later trace and report. |
| `snapreq.request.method`, `.url`, `.body` | Read in both phases; assignable in pre-request scripts. These are the file's values with `{{placeholders}}` still in them: what a script writes is filled in too. Bodies are text or JSON (objects are serialized). |
| `snapreq.request.headers.get/has/set/add/remove/all/toObject` | The request's own headers (inherited ones are added when the request is built). |
| `snapreq.response.status`, `.time` (ms), `.body`, `.text()`, `.json()`, `.headers.get(name)` | Test scripts only. |
| `snapreq.test(name, fn)` | A check named `test <name>`: it passes unless `fn` throws. Shown and reported like `[assert]` checks; a failed one fails the request. |
| `snapreq.expect(value)` | Assertions in Jest style (`toBe`, `toEqual`, `toContain`, `toBeGreaterThan`, `toMatch`, `toHaveProperty`, `toHaveLength`, `toBeTypeOf`, `toBeTruthy`, …, with `.not`) or Chai style (`to.equal`, `to.eql`, `to.include`, `to.be.above`, `to.have.property`, `to.have.lengthOf`, `to.be.a("string")`, `to.be.ok`, `to.exist`, …, with `.not` and `.deep`). |
| `console.log(...)` (and `info`, `warn`, `error`) | Output shown with the result (`-v` for passing requests in `snapreq run`); secret values are hidden. |
| `snapreq.phase` | `"pre_request"` or `"test"`. |

Scripts run in a sandbox (ADR-031): only the JavaScript language itself (ES2023, no modules, no
`require`, `fetch`, files, timers or processes), a fresh engine for every script, at most 2 seconds,
32 MiB of memory and a bounded stack. Tests are synchronous.

### WebSocket requests

A request file with a `ws://` or `wss://` URL, or with a `[websocket]` table, is a WebSocket request
(ADR-033). It has no `method` or `[body]`; `[query]`, `[headers]`, `[auth]` and `[settings]` apply to the
opening handshake, and `[script]` pre-request scripts run before it. Saved messages are sent from the UI
or with `snapreq ws --message NAME`:

```toml
url = "{{ws_base}}/chat"            # an http(s) URL works too when `[websocket]` is present

[websocket]
subprotocols = ["chat.v2"]          # offered in Sec-WebSocket-Protocol

[[websocket.messages]]
name = "join"
kind = "json"                       # text (default), json (checked to be JSON) or binary (base64)
content = '{"room": "{{room}}", "token": "{{token}}"}'
```

Placeholders in messages are resolved like the rest of the file (secret values are hidden wherever messages
are shown). `snapreq run` and flows leave WebSocket requests out.

### gRPC requests

A request file with a `grpc://` (plaintext HTTP/2) or `grpcs://` (TLS) URL, or with a `[grpc]` table, is a
gRPC call (ADR-036). It has no `method` or `[body]`; `[headers]` are sent as metadata, `[auth]` (basic,
bearer, header API key, OAuth 2.0) adds its header, and `[settings]` apply (TLS, proxy, timeouts; the total
timeout becomes the call's deadline). Services are described by server reflection, or by proto files:

```toml
url = "grpc://{{grpc_host}}"

[headers]
x-tenant = "acme"

[grpc]
method = "shop.v1.Orders/Get"           # package.Service/Method
protos = ["protos/shop.proto"]          # optional; relative to this file (default: server reflection)
import_paths = ["protos"]               # optional
message = '{"id": "{{order_id}}"}'      # proto3 JSON; or `messages = [...]` for client streaming
```

Placeholders work in the method, paths and messages. `snapreq run` and flows leave gRPC requests out.

## `folder.toml`

Optional. Same keys as `snapreq.toml` except `format`: `name`, `description`, `order`, `[vars]`,
`[headers]`, `[auth]`, `[settings]`, `[script]`.

## Variables

`[vars]` tables (in `snapreq.toml`, `folder.toml` and request files) and environment files hold
variables. Values are strings; numbers and booleans are accepted and turned into strings.

```toml
[vars]
base_url = "http://localhost:3000"
page_size = 50
token = { secret = true }                       # a secret: the value is never stored here
```

Names may contain letters, digits, `_`, `-` and `.` (quote names with dots: `"api.key" = "…"`).
Names starting with `$` are reserved for built-in dynamic variables.

When a request runs, a `{{name}}` placeholder takes the value from the first of these scopes that
defines `name`:

1. **runtime**: `--var name=value` on the command line, and values captured from earlier responses
2. **request**: the request file's `[vars]`
3. **folders**: each enclosing folder's `[vars]`, innermost first
4. **environment**: the selected environment
5. **collection**: `snapreq.toml`'s `[vars]`
6. **global**: user-wide variables

Collection variables are defaults; the selected environment overrides them; folder and request
variables are deliberate local overrides.

### Placeholders

- `{{name}}` is replaced by the variable's value; spaces inside the braces are allowed (`{{ name }}`).
- Values may contain placeholders themselves (`base_url = "https://{{host}}"`); variables that refer
  to each other in a loop are an error.
- Placeholders work in the URL, query, headers, auth values, body content, form and multipart values,
  file paths and the proxy setting. Disabled entries are left as written.
- Built-in dynamic values, generated anew for every placeholder: `{{$uuid}}`, `{{$timestamp}}`
  (Unix seconds), `{{$isoTimestamp}}` (RFC 3339, UTC) and `{{$randomInt}}` (0 to 1000).
- `\{{` produces a literal `{{`.
- An undefined variable is an error naming the file, the field (for example `headers.Authorization`)
  and the scopes that were searched.
- Values of secret variables are hidden in every trace and output.

## Environments

Each file in `environments/` is one environment, named after the file (`environments/prod.toml`
is `prod`). It is a flat table of variables in the same form as `[vars]`:

```toml
base_url = "https://api.example.com"
token = { secret = true }
```

Secret values are never written into collection files, so the collection can be committed safely.

## Secrets

A variable declared `{ secret = true }` gets its value from the first of:

1. `--var name=value` on the command line;
2. the environment variable `SNAPREQ_SECRET_<NAME>` (upper case; `.` and `-` become `_`, so
   `api.key` is `SNAPREQ_SECRET_API_KEY`), the natural choice in CI;
3. `.secrets/<environment>.toml` inside the collection;
4. `.secrets/shared.toml` inside the collection.

The files in `.secrets/` are flat `name = "value"` tables. SnapRequest creates them (for example with
`snapreq secret set`), writes a `.secrets/.gitignore` containing `*` so they are never committed
even if the project's `.gitignore` forgets them, and makes them readable by their owner only on
Unix. Secret values are hidden in every trace and output.

## Inheritance

Headers, auth and settings flow from `snapreq.toml`, through each enclosing `folder.toml` (outermost first),
down to the request file.

- **Headers:** inherited headers come first. A header with the same name (case-insensitive) as an
  inherited one replaces all inherited headers of that name. A disabled entry
  (`X-Trace = { value = "on", enabled = false }`) switches the inherited header off for this folder
  or request.
- **Auth:** the innermost `[auth]` that is not `inherit` applies; `none` switches it off. Only the auth
  that applies is interpolated, so an overridden one may use variables that are not defined.
- **Settings:** each key overrides the inherited value; keys that are not set are inherited. Relative
  paths (`ca_cert`, `client_cert`, `client_key`) resolve against the directory of the file that set them.
- **Variables:** see "Variables" for how `[vars]` tables combine.

## Flows

A **flow** (`*.flow.toml`, anywhere in the collection) is a workflow of chained requests: its steps run
in order, values captured by one step are available to the next, and the flow stops at the first
step that fails. Its key is its path without `.flow.toml` (`users/onboarding`).

```toml
name = "Checkout"                      # optional
description = "Log in, order, wait for payment, clean up."

[vars]                                 # defaults for every step
item = "sku-42"

[[step]]
request = "auth/login"                 # a request of the collection, with its own [assert]/[capture]
[step.capture]
token = { json = "$.access_token", secret = true }

[[step]]
name = "Create order"                  # or an inline request: every request-file key works
method = "POST"
url = "{{base_url}}/orders"
[step.body]
type = "json"
content = '{ "item": "{{item}}" }'
[step.assert]
status = 201
[step.capture]
order_id = "$.id"

[[step]]
request = "orders/get"                 # uses {{order_id}}
vars = { expand = "items" }            # variables for this step only
delay_ms = 200                         # wait before the step
[step.retry]                           # poll: send again until the checks pass
max = 20                               # attempts in total (default 10)
every_ms = 500                         # pause between attempts (default 1000)
[step.retry.until.json]                # optional; default: the step's own checks
"$.state" = "paid"
```

- A step has either `request` (a collection request key) or `url` (an inline request). A `request`
  step cannot change the request's method, headers, body, auth or settings; use an inline step.
- Inline steps inherit headers, auth and settings like a request file in the flow's folder
  (`users/folder.toml` for `users/onboarding.flow.toml`), and resolve relative paths against the flow.
- Checks: a `request` step runs the request file's `[assert]` and `[capture]` plus the step's own.
- Variables, highest first: `--var` → step `vars` → values captured earlier in the run → the flow's
  `[vars]` → the request's usual scopes (request, folders, environment, collection).
- Retries: a step with `retry` is sent again while the `until` checks (or, without `until`, any of its
  checks or captures) fail, and also after network errors, up to `max` attempts. A step that cannot
  be built (an undefined variable, say) is not retried. Write `[step.retry]` as a table when it has
  `until` (TOML cannot add tables to an inline `retry = { ... }`).
- A step passes when it got a response, every check passed and every capture found its value.

## Ordering

Collections and folders list their children in `order` (by key). Items not listed follow in
alphabetical order. Keys listed in `order` that do not exist produce a warning, not an error.

## Versioning

`format` in `snapreq.toml` is the version of this specification. SnapRequest refuses collections
with a newer `format` than it understands, and will read older formats forever.

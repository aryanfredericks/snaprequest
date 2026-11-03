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
│   └── create-user.json      # a body file used by create-user.toml
└── environments/             # environments (see "Environments")
    ├── dev.toml
    └── prod.toml
```

## Rules

- Every `*.toml` file in the collection directory or a folder is a request, except `snapreq.toml`
  and `folder.toml`. Other files (body files, READMEs) are ignored.
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

`content` and `file` cannot both be set. Multipart parts:

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
| `"api_key"` | `name`, `value`, optional `location` | header `name: value`, or with `location = "query"` the query parameter `name=value` |

```toml
[auth]
type = "api_key"
name = "X-API-Key"
value = "{{api_key}}"
location = "header"                # "header" (default) | "query"
```

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

## `folder.toml`

Optional. Same keys as `snapreq.toml` except `format`: `name`, `description`, `order`, `[vars]`,
`[headers]`, `[auth]`, `[settings]`.

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

## Ordering

Collections and folders list their children in `order` (by key). Items not listed follow in
alphabetical order. Keys listed in `order` that do not exist produce a warning, not an error.

## Versioning

`format` in `snapreq.toml` is the version of this specification. SnapRequest refuses collections
with a newer `format` than it understands, and will read older formats forever.

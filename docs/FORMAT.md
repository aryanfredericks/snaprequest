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
`[headers]`, `[settings]`.

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

## Environments

Each file in `environments/` is one environment, named after the file (`environments/prod.toml`
is `prod`). It is a flat table of variables in the same form as `[vars]`:

```toml
base_url = "https://api.example.com"
token = { secret = true }
```

Secret values are never written into collection files; they are supplied separately (see the CLI
documentation) so the collection can be committed safely.

## Ordering

Collections and folders list their children in `order` (by key). Items not listed follow in
alphabetical order. Keys listed in `order` that do not exist produce a warning, not an error.

## Versioning

`format` in `snapreq.toml` is the version of this specification. SnapRequest refuses collections
with a newer `format` than it understands, and will read older formats forever.

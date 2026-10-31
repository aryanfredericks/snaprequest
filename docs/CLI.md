# SnapRequest CLI

```
snapreq send [METHOD] URL [OPTIONS]
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

Only one kind of body can be given per request.

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

## Output options

| Option | Meaning |
|---|---|
| `-p, --print PARTS` | What to print: `H` request headers (as sent, credentials redacted), `B` request body, `h` status line and response headers, `b` response body, `m` timings and sizes. Default: `hb` on a terminal, `b` when piped, so `snapreq send URL \| jq` works. |
| `--color auto\|always\|never` | Colors (default `auto`: only on a terminal, and never when `NO_COLOR` is set). |
| `-v` / `-vv` | Show the trace on **stderr** (stdout keeps only the `--print` parts). See below. |
| `-f, --fail` | Exit with code 4 when the status is 400 or higher. |
| `--secret VALUE` | Hide this value everywhere in the output (repeatable). Credentials in `Authorization`, `Cookie` and similar headers are hidden automatically. |

## Exit codes

| Code | Meaning |
|---|---|
| 0 | A response was received (any status, unless `--fail`). |
| 1 | Unexpected internal error. |
| 2 | Invalid command line (unknown option, missing URL). |
| 3 | Invalid request: bad URL or option value, unreadable file. Nothing was sent. |
| 4 | `--fail` was given and the status was 400 or higher. |
| 5 | Network failure: DNS, connection, TLS or protocol error. |
| 6 | A connect or total timeout fired. |

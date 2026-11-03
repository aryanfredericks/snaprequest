# SnapRequest

**Fast, open-source, self-hosted HTTP API testing and debugging.**

SnapRequest is a single Rust binary that gives you:

- **A CLI** for sending requests and running test collections in CI (see [docs/CI.md](docs/CI.md)).
- **A self-hosted web UI** (`snapreq serve`) for building and debugging requests in the browser.
- **Detailed traces** for every request: DNS, TCP connect, TLS handshake (version, cipher, certificates),
  time to first byte, download time, redirects and the raw bytes on the wire.
- **Git-friendly collections**: plain TOML files, one per request.

No cloud account. No telemetry.

> **Status:** early development. The HTTP engine, the CLI, collections (environments, variables, secrets,
> auth helpers, checks, flows, data-driven runs) and the self-hosted server with its web UI work; multi-user
> accounts and hardening are next. See [`docs/CLI.md`](docs/CLI.md), [`docs/FORMAT.md`](docs/FORMAT.md),
> [`docs/WEB.md`](docs/WEB.md) and [`docs/SERVER.md`](docs/SERVER.md). See [`docs/PLAN.md`](docs/PLAN.md)
> for the roadmap and [`docs/CHECKPOINT.md`](docs/CHECKPOINT.md) for current progress.

## Building from source

Requires a recent stable Rust toolchain (see [rustup.rs](https://rustup.rs)) and, for the web UI,
Node.js 24.

```sh
npm --prefix web ci && npm --prefix web run build   # the web UI, embedded into the binary
cargo build --release
./target/release/snapreq serve                      # then open http://127.0.0.1:7878
```

Or with Docker: `docker compose up -d` (see [`docs/SERVER.md`](docs/SERVER.md)).

## Project layout

| Path | Contents |
|---|---|
| `crates/snapreq-core` | HTTP engine, request/response model, trace events |
| `crates/snapreq-collection` | Collection and environment file format, variables |
| `crates/snapreq-test` | Assertions, collection runner, reporters |
| `crates/snapreq-server` | Self-hosted server: API, storage, auth |
| `crates/snapreq-cli` | The `snapreq` binary |
| `web/` | The web UI (Svelte 5 + TypeScript), embedded into the server |
| `docs/` | Plan, progress checkpoints and design decisions |

## Contributing

Contributions are welcome. Please read [CONTRIBUTING.md](CONTRIBUTING.md) first.

## License

[MIT](LICENSE)

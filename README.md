# SnapRequest

**Fast, open-source, self-hosted HTTP API testing and debugging.**

SnapRequest is a single Rust binary that gives you:

- **A CLI** for sending requests and running test collections in CI.
- **A self-hosted web UI** (`snapreq serve`) for building and debugging requests in the browser.
- **Detailed traces** for every request: DNS, TCP connect, TLS handshake (version, cipher, certificates),
  time to first byte, download time, redirects and the raw bytes on the wire.
- **Git-friendly collections**: plain TOML files, one per request.

No cloud account. No telemetry.

> **Status:** early development. The HTTP engine and the `snapreq send` CLI work; collections and
> the self-hosted server are next. See [`docs/CLI.md`](docs/CLI.md) for usage. See [`docs/PLAN.md`](docs/PLAN.md)
> for the roadmap and [`docs/CHECKPOINT.md`](docs/CHECKPOINT.md) for current progress.

## Building from source

Requires a recent stable Rust toolchain (see [rustup.rs](https://rustup.rs)).

```sh
cargo build --release
./target/release/snapreq --version
```

## Project layout

| Path | Contents |
|---|---|
| `crates/snapreq-core` | HTTP engine, request/response model, trace events |
| `crates/snapreq-collection` | Collection and environment file format, variables |
| `crates/snapreq-test` | Assertions, collection runner, reporters |
| `crates/snapreq-server` | Self-hosted server: API, storage, auth |
| `crates/snapreq-cli` | The `snapreq` binary |
| `docs/` | Plan, progress checkpoints and design decisions |

## Contributing

Contributions are welcome. Please read [CONTRIBUTING.md](CONTRIBUTING.md) first.

## License

[MIT](LICENSE)

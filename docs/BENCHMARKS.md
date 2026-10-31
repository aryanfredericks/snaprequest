# Benchmarks

Engine benchmarks live in `crates/snapreq-core/benches/client.rs` and use
[criterion](https://github.com/bheisler/criterion.rs). They measure SnapRequest's own overhead
(building the request, tracing every phase, pooling, decoding): the test server runs on loopback and
answers immediately, so network latency is not part of the numbers.

```sh
cargo bench -p snapreq-core                     # full run
cargo bench -p snapreq-core -- http1/           # one group
```

CI only compiles the benchmarks (through `cargo clippy --all-targets`); it does not run them.

## Results

2026-10-31, release build, Linux (Fedora), AMD Ryzen 7 7800X3D (8 CPUs available), 30 GiB RAM.
Values are criterion's median estimate.

| Benchmark | Time | Notes |
|---|---|---|
| `http1/get_keep_alive` | 26 µs | Pooled connection, full trace recorded |
| `http1/get_new_connection` | 72 µs | TCP connect + HTTP/1.1 handshake every time |
| `http1/post_1mib` | 1.25 ms | ≈ 800 MiB/s upload, server drains the body |
| `https/get_keep_alive` | 28 µs | Pooled TLS 1.3 connection |
| `http2/16_concurrent_gets` | 148 µs | 16 multiplexed streams, ≈ 9 µs per request |

## Findings so far

- **Happy Eyeballs timer (fixed in P1.9).** The first connection attempt used to wait on
  `tokio::time::sleep(0)`, which still waits for the next 1 ms timer tick. Every new connection paid
  about 1 ms for nothing. Starting the first attempt immediately took `get_new_connection` from 1.14 ms
  to 72 µs.
- **Servers without `TCP_NODELAY`.** With Nagle's algorithm on in the *server*, HTTP/2 responses
  hit the delayed-ACK interaction and took about 40 ms. SnapRequest always sets `TCP_NODELAY` on its own sockets;
  the benchmark servers do too.

A comparison with curl and other clients is planned for the 1.0 release (P11.4).

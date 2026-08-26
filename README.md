# FDS

Fast Data Transmission. A Linux network engine for TCP, UDP, and SCTP,
written in Rust. The engine is nonblocking, edge-triggered, busy-polled,
batched, and zero-allocation on the hot path.

FDS is the transport layer under Atomos, an HTTP server. It is also a
library for building custom protocols. See the [paper](docs/paper/thesis.pdf)
for the algebraic model that the engine implements, [BENCHMARKS.md](BENCHMARKS.md)
for measured comparisons against existing software, and [WIKI.md](WIKI.md)
for features, architecture, and implementation examples.

## Requirements

- Linux kernel 5.10 or later (io_uring reactor requires kernel 5.19 or later)
- Rust 1.97 or later
- x86-64 with AVX2 for the SIMD checksum path

## Quick start

Build and test:

```sh
cargo test --release
cargo clippy --all-targets -- -D warnings
```

Run the engine. It starts one worker per logical CPU. Each worker owns a
SO_REUSEPORT socket pair and a connection table. The default binds are
UDP 127.0.0.1:7777 and TCP 127.0.0.1:7778.

```sh
cargo run --release -p fds-core
```

Measure the engine:

```sh
# one-way byte ceiling, per direction
cargo run --release -p fds-core -- --bench-large 60000 5
# latency distribution of the loopback datapath
cargo run --release -p fds-core -- --latency 5
# echo throughput against a running engine
cargo run --release -p fds-core -- --bench-udp-against 127.0.0.1:7777 5
cargo run --release -p fds-core -- --bench-tcp-against 127.0.0.1:7778 5
```

### Mini project: full-duplex parallel channels

The example `full_duplex_channels` builds a custom TCP echo server on
the reactor and transport primitives, then opens eight parallel
connections and streams in both directions on each. It shows the
pattern for a custom protocol server: register fds, drain to EAGAIN,
apply write backpressure.

```sh
cargo run --release -p fds-core --example full_duplex_channels
```

Run the benchmark suite against other stacks with the same client,
payload, and settings:

```sh
bash scripts/bench-all.sh
```

See [BENCHMARKS.md](BENCHMARKS.md) for the results and the method.

## Features

- TCP, UDP, and SCTP transports on one epoll reactor
- One worker per logical CPU, pinned, with SO_REUSEPORT flow steering
- recvmmsg and sendmmsg batching (64 datagrams per syscall)
- Zero allocation in the datapath, enforced by a counting allocator
- Preallocated per-core connection tables with hot and cold cache lines
- Checksums in AVX2, with a scalar fallback
- io_uring reactor (feature `io-uring`), opt-in
- AF_XDP frame pipeline (feature `af-xdp`), device-gated
- Runtime configuration through `config.json` and `FDS_*` environment
  variables
- Deterministic fuzz harness for the parsers and checksums

## Documentation

- [WIKI.md](WIKI.md): features, architecture, implementation examples,
  and kernel and NIC tuning for production deployment
- [BENCHMARKS.md](BENCHMARKS.md): apples-to-apples measurements
- [docs/paper/thesis.pdf](docs/paper/thesis.pdf): the algebraic model of
  stateful transformations that the engine implements

## License

MIT. Copyright (c) 2026 Alex @AscendNoosphere, XENOT Corporation.

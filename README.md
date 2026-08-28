# FDS

FDS is Fast Data Transmission. It is a Linux network engine for TCP, UDP, and SCTP. The engine is written in Rust. The hot path is nonblocking, edge-triggered, event-driven, batched, and zero-allocation. You can enable a busy-poll spin for dedicated cores.

The algebraic model is in the [thesis](Docs/paper/thesis.pdf). Measured comparisons are in [Docs/benchmarks.md](Docs/benchmarks.md). Architecture and operations are in the [wiki](Docs/wiki/README.md).

## Requirements

- Linux kernel 5.10 or later. The io_uring reactor needs kernel 5.19 or later.
- Rust 1.97.1 or later.
- x86-64 with AVX2 for the SIMD checksum path.

## Quick start

```sh
cd Code
cargo test --release
cargo clippy --all-targets -- -D warnings
cargo run --release -p fds-engine
```

The engine starts one worker per logical CPU. Default binds are UDP 127.0.0.1:7777 and TCP 127.0.0.1:7778.

```sh
# one-way byte ceiling, per direction
cargo run --release -p fds-engine -- --bench-large 60000 5
# latency of the loopback datapath
cargo run --release -p fds-engine -- --latency 5
```

A custom protocol example:

```sh
cargo run --release -p fds --example full_duplex_channels
```

Full instructions: [Docs/getting-started.md](Docs/getting-started.md).

## License

Apache License 2.0. Copyright 2026 XENOT Corporation.

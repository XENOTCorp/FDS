# FDS

FDS is Fast Data Transmission. A Linux network engine for TCP, UDP, and SCTP. Rust. Nonblocking, edge-triggered, event-driven, batched, zero-allocation. Busy-poll is optional for dedicated cores.

Thesis: [Docs/paper/thesis.pdf](Docs/paper/thesis.pdf). Numbers: [Docs/benchmarks.md](Docs/benchmarks.md). Wiki: [Docs/wiki/README.md](Docs/wiki/README.md).

## Crates

| Crate | Role |
| --- | --- |
| `mol` | Atom and molecule framework |
| `fds` | Transport library |
| `fds-engine` | Echo engine binary `fds` |
| `fds-detect` | Hardware detect and `config.json` |

Workspace is `Code/`.

## Requirements

- Linux 5.10+. io_uring reactor needs 5.19+.
- Rust 1.97.1+.
- x86-64 with AVX2.
- `libsctp-dev`, `liburing-dev` for default features.

## Quick start

```sh
cd Code
cargo test --release
cargo clippy --all-targets -- -D warnings
cargo run --release -p fds-engine
```

One worker per logical CPU. UDP 127.0.0.1:7777. TCP 127.0.0.1:7778.

```sh
cargo run --release -p fds-engine -- --bench-large 60000 5
cargo run --release -p fds-engine -- --latency 5
cargo run --release -p fds --example full_duplex_channels
```

[Docs/getting-started.md](Docs/getting-started.md).

## License

Apache License 2.0. Copyright 2026 XENOT Corporation.

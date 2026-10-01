# FDS

A Linux networking toolkit in Rust: nonblocking TCP/UDP transports,
edge-triggered epoll, batched I/O, and preallocated connection state.
The workspace also includes SCTP, experimental io_uring and AF_XDP
backends, a userspace TCP prototype, and a loopback echo/benchmark engine.

**Status:** pre-1.0 research software. APIs may change. The echo engine is
an example and benchmark target, not a production server.

## Quick start

Requirements: Linux, x86-64, and Rust 1.97.1 (pinned in
`Code/rust-toolchain.toml`). Rustup installs the pinned toolchain when you
enter `Code/` and invoke Cargo.

Start with the minimal TCP/UDP build; no SCTP development library or
liburing is needed:

```sh
cd Code
cargo build --release --locked -p fds-engine --no-default-features
FDS_CORE_THREADS=2 ./target/release/fds
```

UDP echo listens on `127.0.0.1:7777`, TCP on `127.0.0.1:7778`.
Stop with Ctrl-C. Runtime settings come from `config.json` and supported
`FDS_*` environment overrides. Without `FDS_CORE_THREADS`, the engine
starts one worker per logical CPU.

For all default features, install libsctp first (Debian/Ubuntu):

```sh
sudo apt-get install libsctp-dev
cargo build --release --locked --workspace
```

io_uring uses a Rust dependency and does **not** link liburing. SCTP
requires kernel support; AF_XDP requires appropriate device setup and
privileges. See [datapaths](Docs/wiki/datapaths.md) for backend details.

For a host-tuned build:

```sh
bash build/build.sh --release
# Build for a fixed CPU baseline without adding host-only SIMD features:
TARGET_CPU=x86-64-v3 bash build/build.sh --release
```

A `native` binary may not run on another CPU. See the
[build reference](Docs/wiki/build.md).

## Capabilities and limits

- TCP and UDP: IPv4, IPv6, dual-stack, SO_REUSEPORT, batched send/receive.
- epoll: edge-triggered readiness with preallocated event storage;
  handlers must drain to `EAGAIN`.
- `fds::api`: readiness-driven Driver and `poll_*` interfaces. These are
  not drop-in Tokio traits; callers must arrange readiness and wakeups.
- `mol`: fixed buffers, bounded rings, pool guards, and composable
  state transformations. Setup may allocate; hot-path allocation checks
  cover specific tested paths, not every backend.
- Optional `sctp`, `io-uring`, and `af-xdp` features can be disabled on
  either the library or engine.

The epoll TCP echo handler uses one reusable 64 KiB buffer per
connection and pauses reads on write backpressure. UDP zero-copy echo is
disabled pending owned-buffer/send-ID tracking; the low-level send API
is explicitly unsafe. The userspace TCP and kernel-bypass backends still
need protocol, lifetime, and privileged-hardware validation before
production use. See the [review notes](Docs/code-review.md).

[Benchmark snapshots](Docs/benchmarks.md) include historical results.
They are workload- and host-specific, not evidence of universal or
state-of-the-art performance. Re-run comparisons on your target hardware.

## Development

From `Code/`:

```sh
cargo fmt --all --check
cargo test --workspace --locked --no-default-features
cargo test --workspace --release --locked --no-default-features
cargo clippy --workspace --all-targets --locked --no-default-features -- -D warnings
cargo test -p mol --locked -- --ignored --test-threads=1
bash build/test-build.sh
```

With libsctp installed, repeat tests and Clippy with `--all-features`
instead of `--no-default-features`. CI covers both configurations.

## Repository

| Path | Contents |
| --- | --- |
| `Code/crates/mol` | Buffers, rings, pools, atom/molecule framework |
| `Code/crates/fds` | Transport library and public API |
| `Code/crates/fds-engine` | Echo engine and benchmark CLI (`fds`) |
| `Code/crates/fds-detect` | Hardware detection and config/schema tools |
| `Code/build`, `Code/scripts` | Build tooling and benchmark scripts |
| `Docs` | Guides, benchmark evidence, engineering standard, thesis |

- [Getting started](Docs/getting-started.md)
- [Architecture and operations](Docs/wiki/README.md)
- [API migration notes](Docs/migration.md)
- [Engineering standard](Docs/standard/standard.md)
- [Thesis](Docs/paper/thesis.pdf)
- [Contributing](CONTRIBUTING.md) · [Security](SECURITY.md)

## License

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).

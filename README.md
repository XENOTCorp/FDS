# FDS

FDS is a Linux network engine for TCP, UDP, and SCTP. It is written in Rust.

The engine is nonblocking, edge-triggered, event-driven, batched, and
zero-allocation on the hot path. Busy-poll is optional for dedicated cores.

FDS detects the host hardware and adapts the build to it. One source tree
runs on any supported Linux system.

## Features

- TCP, UDP, and SCTP transports
- Public API: Driver/callback and AsyncRead/AsyncWrite (`fds::api`)
- epoll, io_uring (multishot recv/accept, registered buffers, SEND_ZC),
  and AF_XDP (native zero-copy, NUMA-local rings) datapaths
- One worker per logical CPU
- SO_REUSEPORT flow steering across workers
- Batched receive and send (recvmmsg, sendmmsg, readv, writev)
- Zero allocation on the hot path
- Adaptive build: `build/build.sh` detects the CPU and sets the codegen flags
- Hardware detection: `fds-detect` writes the host `config.json`

## Requirements

- Linux kernel 5.10 or later. The io_uring reactor needs kernel 5.19 or
  later.
- Rust 1.97.1 or later. The file `rust-toolchain.toml` pins the version.
  Rustup installs the pinned toolchain automatically.
- x86-64. The SIMD checksum path uses AVX2 when the CPU has it. Detection is
  at runtime. Other CPUs use the scalar path.
- The development packages for libsctp and liburing. These are required for
  the default features. Install them with the package manager of your
  distribution. On Debian or Ubuntu:

  ```sh
  sudo apt-get install -y libsctp-dev liburing-dev
  ```

## Quick start

```sh
cd Code
bash build/build.sh --release
./target/release/fds
```

The build script detects the host, sets the codegen flags, and builds the
workspace. See [Docs/wiki/build.md](Docs/wiki/build.md) for the full build
reference.

Run the tests:

```sh
cargo test --release
cargo clippy --all-targets -- -D warnings
```

Both commands must exit 0.

The engine starts one worker per logical CPU. UDP echo binds to
127.0.0.1:7777. TCP echo binds to 127.0.0.1:7778.

Stop the process with Ctrl-C.

## Documentation

- [Getting started](Docs/getting-started.md): install, test, and run
- [Wiki](Docs/wiki/README.md): architecture, datapaths, configuration,
  operations, examples, applications
- [Benchmarks](Docs/benchmarks.md): measured comparisons with other network
  stacks
- [Thesis](Docs/paper/thesis.pdf): the mathematical foundation of the
  zero-allocation dataplane
- [Standard](Docs/standard/standard.md): the engineering standard for the
  Atom/Molecule architecture

## Repository layout

```
Code/              Rust workspace (mol, fds, fds-engine, fds-detect)
Code/build/        Adaptive build: build.sh, detect.sh
Code/scripts/      Benchmark and kernel-bypass test scripts
Code/config/       config.json schema
Docs/              Documentation, benchmarks, thesis, standard
```

## Crates

| Crate | Role |
| --- | --- |
| `mol` | Atom and molecule framework |
| `fds` | Transport library and public API |
| `fds-engine` | Echo engine binary `fds` |
| `fds-detect` | Hardware detection and `config.json` tooling |

The workspace is `Code/`. The detailed module map is in the
[architecture wiki page](Docs/wiki/architecture.md).

## License

Apache License 2.0. Copyright 2026 XENOT Corporation. See [LICENSE](LICENSE)
and [NOTICE](NOTICE).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## Security

See [SECURITY.md](SECURITY.md).
